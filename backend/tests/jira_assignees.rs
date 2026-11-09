mod common;
use common::*;
use pullboard::{
    cache::{Cache, SourceUpdate},
    config::Upstream,
    jira::Jira,
    model::{Kind, Purpose},
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

async fn account_response(server: &MockServer, id: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path("/rest/api/3/user/assignable/search"))
        .and(query_param("accountId", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn configured_group_checks_known_account_beyond_first_thousand_without_broad_enumeration() {
    let server = MockServer::start().await;
    mock_get(&server, "/rest/api/3/myself", jira_user("user-1005")).await;
    mock_get(
        &server,
        "/rest/api/3/group/bulk",
        json!({"startAt":0,"total":1,"values":[{"name":"restricted","groupId":"real-group"}]}),
    )
    .await;
    mock_get(&server,"/rest/api/3/group/member",json!({"startAt":0,"total":2,"values":[jira_user("user-1005"),jira_user("not-assignable")]})).await;
    // The legacy filtered range can be empty even when a known group member is
    // assignable beyond its first-1000-user scan. It must never be consulted.
    Mock::given(method("GET"))
        .and(path("/rest/api/3/user/assignable/search"))
        .and(query_param_is_missing("accountId"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(0)
        .mount(&server)
        .await;
    account_response(&server, "user-1005", json!([jira_user("user-1005")])).await;
    account_response(&server, "not-assignable", json!([])).await;
    let mut c = jira(&server.uri());
    c.assignee_groups = vec!["restricted".into()];
    let j = Jira::new(&c, &Upstream::default()).await.unwrap();
    j.identity().await.unwrap();
    let pool = j
        .discover_assignees("DEV", Purpose::Implementation, "")
        .await
        .unwrap();
    assert_eq!(
        pool.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["user-1005"]
    );
    let log = server.received_requests().await.unwrap();
    assert!(
        !log.iter()
            .any(|r| r.url.path() == "/rest/api/3/users/search"
                || r.url.path() == "/rest/api/3/user/search")
    );
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
            .count(),
        2
    );
}

#[tokio::test]
async fn absent_and_empty_groups_enumerate_all_users_past_thousand_and_check_every_candidate() {
    for explicit_empty in [false, true] {
        let server = MockServer::start().await;
        mock_get(&server, "/rest/api/3/myself", jira_user("user-1004")).await;
        let users: Vec<_> = (0..1005)
            .map(|i| jira_user(&format!("user-{i:04}")))
            .collect();
        mock_all_users(&server, users.clone()).await;
        mock_assignable(&server, users).await;
        // Empty individual results remove only that candidate, never terminate
        // the pool scan. Old range enumeration is forbidden by the mock helper.
        account_response(&server, "user-0000", json!([])).await;
        let yaml = format!(
            "base_url: {}\nemail: me@example.invalid\ntoken: {{value: mock}}\nprojects: [DEV]\nboard_id: 12\nteam: Platform\nsprint_name_pattern: literal\n{}",
            server.uri(),
            if explicit_empty {
                "assignee_groups: []\n"
            } else {
                ""
            }
        );
        let c = serde_yaml::from_str(&yaml).unwrap();
        let j = Jira::new(&c, &Upstream::default()).await.unwrap();
        j.identity().await.unwrap();
        let pool = j
            .discover_assignees("DEV", Purpose::Implementation, "")
            .await
            .unwrap();
        assert_eq!(pool.len(), 1004);
        assert!(pool.iter().any(|u| u.id == "user-1004"));
        let log = server.received_requests().await.unwrap();
        assert!(log.iter().any(|r| {
            r.url.path() == "/rest/api/3/users/search"
                && r.url
                    .query_pairs()
                    .any(|(k, v)| k == "startAt" && v == "1000")
        }));
        assert_eq!(
            log.iter()
                .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
                .count(),
            1005
        );
        assert!(
            log.iter()
                .all(|r| r.method == "GET" && r.url.path() != "/rest/api/3/user/search")
        );
        assert!(
            log.iter()
                .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
                .all(|r| r.url.query_pairs().any(|(k, _)| k == "accountId")
                    && !r
                        .url
                        .query_pairs()
                        .any(|(k, _)| k == "startAt" || k == "query"))
        );
    }
}

#[tokio::test]
async fn all_users_inactive_apps_and_empty_individual_results_do_not_hide_later_humans() {
    let server = MockServer::start().await;
    mock_get(&server, "/rest/api/3/myself", jira_user("later")).await;
    let mut inactive = jira_user("inactive");
    inactive["active"] = json!(false);
    let mut app = jira_user("app");
    app["accountType"] = json!("app");
    mock_all_users(
        &server,
        vec![
            inactive,
            app,
            jira_user("not-assignable"),
            jira_user("later"),
        ],
    )
    .await;
    mock_assignable(&server, vec![jira_user("later")]).await;
    let j = Jira::new(
        &jira(&server.uri()),
        &Upstream {
            page_size: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    j.identity().await.unwrap();
    assert_eq!(
        j.discover_assignees("DEV", Purpose::Implementation, "")
            .await
            .unwrap()[0]
            .id,
        "later"
    );
    let log = server.received_requests().await.unwrap();
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
            .count(),
        2
    );
}

#[tokio::test]
async fn all_users_failed_middle_page_repeated_page_and_exhausted_bounds_fail_closed() {
    for scenario in ["permission", "repeated", "bound", "oversized"] {
        let server = MockServer::start().await;
        mock_get(&server, "/rest/api/3/myself", jira_user("me")).await;
        mock_all_users(&server, vec![jira_user("a"), jira_user("b")]).await;
        if scenario != "bound" {
            let response = match scenario {
                "permission" => {
                    ResponseTemplate::new(403).set_body_string("private upstream error")
                }
                "repeated" => ResponseTemplate::new(200).set_body_json(json!([jira_user("a")])),
                _ => ResponseTemplate::new(200)
                    .set_body_json(json!([jira_user("b"), jira_user("c")])),
            };
            Mock::given(method("GET"))
                .and(path("/rest/api/3/users/search"))
                .and(query_param("startAt", "1"))
                .respond_with(response)
                .with_priority(1)
                .mount(&server)
                .await;
        }
        let j = Jira::new(
            &jira(&server.uri()),
            &Upstream {
                page_size: 1,
                max_pages: if scenario == "bound" { 1 } else { 10 },
                ..Default::default()
            },
        )
        .await
        .unwrap();
        j.identity().await.unwrap();
        let error = j
            .discover_assignees("DEV", Purpose::Implementation, "")
            .await
            .unwrap_err();
        assert_eq!(error.code, "jira_discovery_failed");
        assert!(!error.message.contains("private"));
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path() == "/rest/api/3/user/assignable/search")
        );
    }
}

#[tokio::test]
async fn group_union_candidate_bound_is_explicit_and_never_checks_a_partial_pool() {
    let server = MockServer::start().await;
    mock_get(&server, "/rest/api/3/myself", jira_user("me")).await;
    for (group, user) in [("first", "a"), ("second", "b")] {
        Mock::given(method("GET"))
            .and(path("/rest/api/3/group/bulk"))
            .and(query_param("groupName", group))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"startAt":0,"total":1,"values":[{"name":group,"groupId":group}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/group/member"))
            .and(query_param("groupId", group))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"startAt":0,"total":1,"values":[jira_user(user)]})),
            )
            .mount(&server)
            .await;
    }
    let mut c = jira(&server.uri());
    c.assignee_groups = vec!["first".into(), "second".into()];
    let j = Jira::new(
        &c,
        &Upstream {
            page_size: 1,
            max_pages: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    j.identity().await.unwrap();
    assert_eq!(
        j.discover_assignees("DEV", Purpose::Implementation, "")
            .await
            .unwrap_err()
            .code,
        "jira_discovery_failed"
    );
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/rest/api/3/user/assignable/search")
    );
}

#[tokio::test]
async fn individual_results_must_contain_only_the_single_requested_account() {
    for values in [
        json!([jira_user("stranger")]),
        json!([jira_user("me"), jira_user("stranger")]),
        json!([jira_user("me"), jira_user("me")]),
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        account_response(&server, "me", values).await;
        let j = Jira::new(&jira(&server.uri()), &Upstream::default())
            .await
            .unwrap();
        j.identity().await.unwrap();
        assert_eq!(
            j.discover_assignees("DEV", Purpose::Implementation, "")
                .await
                .unwrap_err()
                .code,
            "jira_discovery_failed"
        );
    }
}

#[tokio::test]
async fn discovery_rate_limits_preserve_backoff_deadline_through_options_and_source_health() {
    for endpoint in [
        "/rest/api/3/myself",
        "/rest/api/3/field",
        "/rest/api/3/issue/createmeta/DEV/issuetypes",
        "/rest/api/3/group/bulk",
        "/rest/api/3/group/member",
        "/rest/api/3/users/search",
        "/rest/api/3/user/assignable/search",
        "/rest/agile/1.0/board/12/sprint",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let mut jc = jira(&server.uri());
        if endpoint.contains("/group/") {
            jc.assignee_groups = vec!["restricted".into()];
            mock_get(
                &server,
                "/rest/api/3/group/bulk",
                json!({"startAt":0,"total":1,"values":[{"name":"restricted","groupId":"real"}]}),
            )
            .await;
            mock_get(
                &server,
                "/rest/api/3/group/member",
                json!({"startAt":0,"total":1,"values":[jira_user("me")]}),
            )
            .await;
        }
        let j = Jira::new(&jc, &Upstream::default()).await.unwrap();
        if endpoint != "/rest/api/3/myself" {
            j.identity().await.unwrap();
        }
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "300"))
            .with_priority(1)
            .mount(&server)
            .await;
        let error = if endpoint == "/rest/api/3/myself" {
            j.identity().await.unwrap_err()
        } else {
            j.discover().await.unwrap_err()
        };
        assert_eq!(error.code, "upstream_backoff");
        let deadline = error.retry_at;
        assert!(deadline.is_some());
        let mut c = config();
        c.jira = Some(jc);
        let cache = Cache::open(&c.server.database, &c).await.unwrap();
        cache
            .verify(identity("jira", Kind::Jira, "me"))
            .await
            .unwrap();
        let s = cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: Err(error),
            }])
            .await
            .unwrap();
        assert_eq!(s.board.sources[0].status, "backoff");
        assert_eq!(s.board.sources[0].retry_at, deadline);
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method == "GET")
        );
    }
}
