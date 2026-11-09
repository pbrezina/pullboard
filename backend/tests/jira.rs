mod common;
use common::*;
use pullboard::{
    cache::{Cache, SourceUpdate},
    config::Upstream,
    jira::{Jira, encode_adf},
    model::Purpose,
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

fn user(id: &str) -> Value {
    json!({"accountId":id,"displayName":id,"emailAddress":format!("{id}@example.invalid"),"accountType":"atlassian","active":true})
}
fn mentioned() -> Value {
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"mention","attrs":{"id":"me","text":"@Me"}},{"type":"text","text":" please check"}]}]})
}
fn upstream_issue(key: &str, assigned: bool, description: Option<Value>) -> Value {
    let project = key.rsplit_once('-').unwrap().0;
    json!({"id":key,"key":key,"fields":{"summary":format!("Issue {key}"),"description":description,"project":{"key":project},"issuetype":{"name":"Task"},"status":{"name":"Open","statusCategory":{"key":"new"}},"reporter":user("other"),"assignee":if assigned{user("me")}else{Value::Null},"created":"2026-01-01T00:00:00Z","updated":"2026-01-05T00:00:00Z","resolution":null}})
}
const ASSIGNED_JQL: &str =
    "project in (\"DEV\") AND assignee = currentUser() AND status != \"Closed\" ORDER BY key ASC";
const MENTION_JQL: &str = "status != \"Closed\" AND (description ~ currentUser() OR comment ~ currentUser()) ORDER BY key ASC";

fn is_issue_history(request: &wiremock::Request) -> bool {
    request.url.path().starts_with("/rest/api/3/issue/")
        && !request
            .url
            .path()
            .starts_with("/rest/api/3/issue/createmeta/")
}

async fn search_page(
    server: &MockServer,
    jql: &str,
    cursor: Option<&str>,
    page_size: u32,
    response: ResponseTemplate,
) {
    let mock = Mock::given(method("GET"))
        .and(path("/rest/api/3/search/jql"))
        .and(query_param("jql", jql))
        .and(query_param("maxResults", page_size.to_string()));
    let mock = match cursor {
        Some(cursor) => mock.and(query_param("nextPageToken", cursor)),
        None => mock.and(query_param_is_missing("nextPageToken")),
    };
    mock.respond_with(response).expect(1).mount(server).await;
}

async fn issue_history(server: &MockServer, key: &str, comments: Vec<Value>, description: bool) {
    let root = format!("/rest/api/3/issue/{key}");
    for (suffix, body) in [
        (
            "comment",
            json!({"total":comments.len(),"comments":comments,"startAt":0,"maxResults":100}),
        ),
        ("remotelink", json!([{"object":{"url":pr().url}}])),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{root}/{suffix}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(server)
            .await;
    }
    if description {
        Mock::given(method("GET"))
            .and(path(format!("{root}/changelog")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"values":[],"isLast":true,"startAt":0,"maxResults":100})),
            )
            .expect(1)
            .mount(server)
            .await;
    }
}
async fn get_json(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn targeted_searches_keep_distinct_cursors_and_deduplicate_ids_before_history_reads() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    get_json(&server, "/rest/api/3/myself", user("me")).await;
    let assigned = upstream_issue("DEV-1", true, Some(mentioned()));
    let mut mention = upstream_issue("OTHER-2", false, None);
    mention["fields"]["assignee"] = user("colleague");
    let mut renamed_duplicate = mention.clone();
    renamed_duplicate["key"] = json!("OTHER-99");
    for (jql, cursor, body) in [
        (
            ASSIGNED_JQL,
            None,
            json!({"issues":[assigned],"isLast":false,"nextPageToken":"assigned-second"}),
        ),
        (
            ASSIGNED_JQL,
            Some("assigned-second"),
            json!({"issues":[assigned],"isLast":true}),
        ),
        (
            MENTION_JQL,
            None,
            json!({"issues":[mention],"isLast":false,"nextPageToken":"mention-second"}),
        ),
        (
            MENTION_JQL,
            Some("mention-second"),
            json!({"issues":[assigned],"isLast":false,"nextPageToken":"mention-third"}),
        ),
        (
            MENTION_JQL,
            Some("mention-third"),
            json!({"issues":[renamed_duplicate],"isLast":true}),
        ),
    ] {
        search_page(
            &server,
            jql,
            cursor,
            1,
            ResponseTemplate::new(200).set_body_json(body),
        )
        .await;
    }
    Mock::given(method("GET")).and(path("/rest/api/3/issue/DEV-1/changelog")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"values":[{"created":"2026-01-02T00:00:00Z","items":[{"field":"description"}]}],"isLast":true,"startAt":0,"maxResults":1}))).expect(1).mount(&server).await;
    for (key, first, second) in [
        (
            "DEV-1",
            json!({"id":"1","body":encode_adf("Old comment"),"author":user("other"),"created":"2026-01-02T00:00:00Z"}),
            json!({"id":"2","body":encode_adf("My answer"),"author":user("me"),"created":"2026-01-03T00:00:00Z"}),
        ),
        (
            "OTHER-2",
            json!({"id":"3","body":encode_adf("My earlier comment"),"author":user("me"),"created":"2026-01-01T00:00:00Z"}),
            json!({"id":"4","body":mentioned(),"author":user("other"),"created":"2026-01-04T00:00:00Z"}),
        ),
    ] {
        for (start, comment) in [(0, first), (1, second)] {
            Mock::given(method("GET"))
                .and(path(format!("/rest/api/3/issue/{key}/comment")))
                .and(query_param("startAt", start.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"comments":[comment],"total":2,"startAt":start,"maxResults":1}),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path(format!("/rest/api/3/issue/{key}/remotelink")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
    }
    let adapter = Jira::new(
        &jira(&server.uri()),
        &Upstream {
            page_size: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let progress = pullboard::sync::Progress::recording("jira");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 2);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(2));
    assert_eq!(data.issues.len(), 2);
    let assigned = data.issues.iter().find(|i| i.key == "DEV-1").unwrap();
    assert!(assigned.is_assigned_to_me);
    assert!(assigned.attention.is_none());
    let mention = data.issues.iter().find(|i| i.key == "OTHER-2").unwrap();
    assert!(!mention.is_assigned_to_me);
    assert_eq!(mention.project_key, "OTHER");
    assert_eq!(mention.assignees[0].id, "colleague");
    assert_eq!(
        mention
            .attention
            .as_ref()
            .unwrap()
            .waiting_since
            .to_rfc3339(),
        "2026-01-04T00:00:00+00:00"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path() == "/rest/api/3/search/jql")
            .count(),
        5
    );
    let last_search = requests
        .iter()
        .rposition(|r| r.url.path() == "/rest/api/3/search/jql")
        .unwrap();
    let first_history = requests.iter().position(is_issue_history).unwrap();
    assert!(last_search < first_history);
    assert!(adapter.projects().await.is_empty()); // Reads alone do not publish choices.
    assert!(adapter.project("OTHER").is_err());
    let mut request = create();
    request.project_key = "OTHER".into();
    assert_eq!(
        adapter.validate_create(&request, &[]).unwrap_err().code,
        "invalid_choice"
    );
}
#[tokio::test]
async fn active_future_sprints_paginate_only_configured_board_keep_foreign_origin() {
    let server = MockServer::start().await;
    let mut c = jira(&server.uri());
    c.sprint_name_pattern = "%YEAR-Identity-S%NUMBER".into();
    for board in [12] {
        for (start, id, state, name) in [
            (0, 101, "active", "2026-CORE-S1"),
            (1, 102, "active", "2026-Identity-S2"),
            (2, 103, "future", "2026-Identity-S3"),
            (3, 104, "active", "Foreign"),
        ] {
            Mock::given(method("GET")).and(path(format!("/rest/agile/1.0/board/{board}/sprint"))).and(query_param("startAt",start.to_string())).and(query_param("state","active,future")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"values":[{"id":id,"name":name,"state":state,"originBoardId":999}],"isLast":start==3,"startAt":start,"maxResults":1}))).expect(1).mount(&server).await;
        }
    }
    let adapter = Jira::new(
        &c,
        &Upstream {
            page_size: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let sprints = adapter.sprints().await.unwrap();
    assert_eq!(sprints.len(), 4);
    assert_eq!(sprints.iter().filter(|s| s.state == "active").count(), 3);
    assert_eq!(sprints.iter().filter(|s| s.default_eligible).count(), 1);
    assert!(
        sprints
            .iter()
            .all(|s| s.team.is_none() && s.board_id == 12 && s.board_url.contains("rapidView=12"))
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path() == "/rest/agile/1.0/board/12/sprint")
    );
}
#[tokio::test]
async fn test_mode_returns_no_fabricated_ids_or_upstream_writes() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&adapter).await;
    let result = adapter.create(&create(), &pr(), &[], true).await.unwrap();
    assert_eq!(result.outcome, "simulated");
    assert!(result.issue.is_none());
    let result = adapter.link("DEV-1", &pr(), false, true).await.unwrap();
    assert_eq!(result.outcome, "simulated");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
#[tokio::test]
async fn post_create_followup_failures_return_real_created_issue_and_warnings() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    get_json(&server, "/rest/api/3/myself", user("me")).await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .and(body_partial_json(
            json!({"fields":{"description":{"type":"doc","version":1}}}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-123"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/DEV-123/remotelink"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    get_json(
        &server,
        "/rest/api/3/issue/DEV-123/transitions",
        json!({"transitions":[]}),
    )
    .await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&adapter).await;
    let mut request = create();
    request.status = Some("Review".into());
    let response = adapter.create(&request, &pr(), &[], false).await.unwrap();
    assert_eq!(response.outcome, "created");
    assert_eq!(response.issue.unwrap().key, "DEV-123");
    assert_eq!(response.warnings.len(), 2);
}

#[tokio::test]
async fn accepted_creation_with_malformed_body_is_not_an_error_inviting_duplicates() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    get_json(&server, "/rest/api/3/myself", user("me")).await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .respond_with(ResponseTemplate::new(201).set_body_string("broken upstream body"))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&adapter).await;
    let result = adapter.create(&create(), &pr(), &[], false).await.unwrap();
    assert_eq!(result.outcome, "created");
    assert!(result.issue.is_none());
    assert!(!result.warnings.is_empty());
    validate_contract(&serde_json::to_value(&result).unwrap(), "CreateIssueResult");
}

#[tokio::test]
async fn upstream_create_server_failure_has_unknown_outcome_not_retryable_duplicate_invitation() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let j = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&j).await;
    assert_eq!(
        j.create(&create(), &pr(), &[], false)
            .await
            .unwrap_err()
            .code,
        "write_outcome_unknown"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method != "GET")
            .count(),
        1
    );
}

#[tokio::test]
async fn unavailable_jira_identity_prevents_real_writes() {
    let server = MockServer::start().await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert_eq!(
        adapter
            .create(&create(), &pr(), &[], false)
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
#[tokio::test]
async fn idempotent_links_use_stable_global_id_and_cached_trusted_url() {
    let server = MockServer::start().await;
    let pr = pr();
    get_json(&server, "/rest/api/3/myself", user("me")).await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/DEV-1/remotelink"))
        .and(body_partial_json(
            json!({"globalId":format!("pullboard:{}",pr.id),"object":{"url":pr.url}}),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&server)
        .await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            adapter
                .link("DEV-1", &pr, false, false)
                .await
                .unwrap()
                .outcome,
            "linked"
        );
    }
    assert_eq!(
        adapter
            .link("DEV-1", &pr, true, false)
            .await
            .unwrap()
            .outcome,
        "already_linked"
    );
}
#[tokio::test]
async fn invalid_project_type_points_sprint_and_team_never_write() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&adapter).await;
    let mut requests = vec![];
    let mut r = create();
    r.project_key = "OTHER".into();
    requests.push(r);
    let mut r = create();
    r.issue_type = "Epic".into();
    requests.push(r);
    let mut r = create();
    r.story_points = Some(-1.0);
    requests.push(r);
    let mut r = create();
    r.sprint_id = Some("999".into());
    requests.push(r);
    let mut r = create();
    r.team = Some("unknown".into());
    requests.push(r);
    let mut r = create();
    r.status = Some(" ".into());
    requests.push(r);
    for r in requests {
        assert!(adapter.create(&r, &pr(), &[], false).await.is_err());
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
#[tokio::test]
async fn options_purpose_defaults_and_paginate_all_users_then_check_account() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let config = jira(&server.uri());
    Mock::given(method("GET"))
        .and(path("/rest/api/3/users/search"))
        .and(query_param("startAt", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([user("zeta")])))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/users/search"))
        .and(query_param("startAt", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/user/assignable/search"))
        .and(query_param("accountId", "zeta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([user("zeta")])))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let adapter = Jira::new(
        &config,
        &Upstream {
            page_size: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ready_jira(&adapter).await;
    let options = adapter
        .options("DEV", Purpose::Review, None, "")
        .await
        .unwrap();
    assert!(options.sprints.is_empty());
    assert_eq!(options.assignees.len(), 1);
    assert_eq!(options.defaults.issue_type.as_deref(), Some("Task"));
    assert!(options.defaults.assignee_id.is_none());
}

#[tokio::test]
async fn unsupported_team_is_explicit_not_silently_omitted() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut j = jira(&server.uri());
    j.team_field_name = "Team".into();
    let mut c = config();
    c.jira = Some(j.clone());
    c.validate().unwrap();
    let adapter = Jira::new(&j, &Upstream::default()).await.unwrap();
    ready_jira(&adapter).await;
    let mut request = create();
    request.team = Some("Platform".into());
    assert_eq!(
        adapter
            .create(&request, &pr(), &[], true)
            .await
            .unwrap_err()
            .code,
        "invalid_choice"
    );
    let options = adapter
        .options("DEV", Purpose::Review, None, "")
        .await
        .unwrap();
    assert_eq!(options.capabilities.team.mode, "unsupported");
    assert!(options.teams.is_empty());
    assert!(
        !options
            .diagnostics
            .iter()
            .any(|d| d.code == "team_default_unavailable")
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn only_closed_is_excluded_and_only_assigned_configured_or_meaningful_mentions_are_cached() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut closed = upstream_issue("DEV-1", true, Some(mentioned()));
    closed["fields"]["status"]["name"] = json!("Closed");
    let mut resolved = upstream_issue("DEV-2", true, None);
    resolved["fields"]["resolution"] = json!({"name":"Fixed"});
    let mut done = upstream_issue("OTHER-3", false, Some(mentioned()));
    done["fields"]["assignee"] = user("colleague");
    done["fields"]["status"] = json!({"name":"Done","statusCategory":{"key":"done"}});
    done["fields"]["resolution"] = json!({"name":"Fixed"});
    let foreign_assigned = upstream_issue("OTHER-4", true, Some(mentioned()));
    let foreign_unmentioned = upstream_issue("OTHER-10", true, None);
    let false_positive = upstream_issue("OTHER-5", false, Some(encode_adf("@Me plain text")));
    let handled = upstream_issue("OTHER-6", false, Some(mentioned()));
    let mut bot = upstream_issue("OTHER-7", false, Some(mentioned()));
    bot["fields"]["reporter"]["accountType"] = json!("app");
    let unassigned = upstream_issue("OTHER-8", false, None);
    let mut closed_mention = upstream_issue("OTHER-9", false, Some(mentioned()));
    closed_mention["fields"]["status"]["name"] = json!("Closed");
    for (jql, issues) in [
        (ASSIGNED_JQL, vec![closed, resolved]),
        (
            MENTION_JQL,
            vec![
                done,
                foreign_assigned,
                foreign_unmentioned,
                false_positive,
                handled,
                bot,
                unassigned,
                closed_mention,
            ],
        ),
    ] {
        search_page(
            &server,
            jql,
            None,
            100,
            ResponseTemplate::new(200).set_body_json(json!({"issues":issues,"isLast":true})),
        )
        .await;
    }
    for key in ["DEV-2", "OTHER-10", "OTHER-5"] {
        issue_history(&server, key, vec![], false).await;
    }
    for key in ["OTHER-3", "OTHER-4", "OTHER-7"] {
        issue_history(&server, key, vec![], true).await;
    }
    issue_history(&server, "OTHER-6", vec![json!({"id":"reply","body":encode_adf("Handled"),"author":user("me"),"created":"2026-01-03T00:00:00Z"})], true).await;
    issue_history(&server, "OTHER-8", vec![
        json!({"id":"mention","body":mentioned(),"author":user("other"),"created":"2026-01-02T00:00:00Z","updated":"2026-01-06T00:00:00Z"}),
        json!({"id":"reply","body":encode_adf("Earlier answer, edited later"),"author":user("me"),"created":"2026-01-03T00:00:00Z","updated":"2026-01-07T00:00:00Z"}),
    ], false).await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = pullboard::sync::Progress::recording("jira");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 8);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(8));
    assert_eq!(
        data.issues
            .iter()
            .map(|i| i.key.as_str())
            .collect::<Vec<_>>(),
        ["DEV-2", "OTHER-3", "OTHER-4", "OTHER-8"]
    );
    assert!(data.issues[0].attention.is_none());
    assert_eq!(data.issues[1].status, "Done");
    assert!(data.issues[1].attention.is_some());
    // An own foreign-project mention remains attention, not Overview eligibility.
    assert!(data.issues[2].is_assigned_to_me);
    assert_eq!(data.issues[2].project_key, "OTHER");
    assert!(data.issues[2].attention.is_some());
    assert!(data.issues[3].assignees.is_empty());
    assert_eq!(
        data.issues[3]
            .attention
            .as_ref()
            .unwrap()
            .waiting_since
            .to_rfc3339(),
        "2026-01-06T00:00:00+00:00"
    );
    assert_eq!(
        data.issue_weblinks
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        ["DEV-2", "OTHER-3", "OTHER-4", "OTHER-8"]
    );
    assert!(data.coverage_notes[0].contains("all accessible projects"));
    assert!(data.coverage_notes[0].contains("Search indexing"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path() == "/rest/api/3/search/jql")
            .count(),
        2
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path().starts_with("/rest/api/3/issue/DEV-1/")
                || r.url.path().starts_with("/rest/api/3/issue/OTHER-9/"))
    );
}

#[tokio::test]
async fn incomplete_issue_hydration_never_increments_progress() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    for jql in [ASSIGNED_JQL, MENTION_JQL] {
        search_page(
            &server,
            jql,
            None,
            100,
            ResponseTemplate::new(200)
                .set_body_json(json!({"issues":[upstream_issue("DEV-1",true,None)],"isLast":true})),
        )
        .await;
    }
    mock_get(
        &server,
        "/rest/api/3/issue/DEV-1/comment",
        json!({"total":0,"comments":[],"startAt":0,"maxResults":100}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/DEV-1/remotelink"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = pullboard::sync::Progress::recording("jira");
    assert!(
        adapter
            .discover_with_progress(progress.clone())
            .await
            .is_err()
    );
    assert_eq!(progress.snapshot().unwrap().completed_items, 0);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(1));
}

#[tokio::test]
async fn empty_targeted_searches_use_all_configured_keys_and_never_fall_back_to_a_scan() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    get_json(
        &server,
        "/rest/api/3/project/OPS",
        json!({"key":"OPS","name":"Operations"}),
    )
    .await;
    get_json(
        &server,
        "/rest/api/3/issue/createmeta/OPS/issuetypes",
        json!({"startAt":0,"maxResults":100,"total":1,"issueTypes":[{"id":"1","name":"Task"}]}),
    )
    .await;
    let fields = create_fields();
    get_json(
        &server,
        "/rest/api/3/issue/createmeta/OPS/issuetypes/1",
        json!({"startAt":0,"maxResults":100,"total":fields.len(),"fields":fields}),
    )
    .await;
    get_json(
        &server,
        "/rest/api/3/project/OPS/statuses",
        json!([{"id":"1","name":"Task","statuses":[]}]),
    )
    .await;
    let mut config = jira(&server.uri());
    config.projects.push("OPS".into());
    for jql in [
        "project in (\"DEV\",\"OPS\") AND assignee = currentUser() AND status != \"Closed\" ORDER BY key ASC",
        MENTION_JQL,
    ] {
        search_page(
            &server,
            jql,
            None,
            100,
            ResponseTemplate::new(200).set_body_json(json!({"issues":[],"isLast":true})),
        )
        .await;
    }
    let adapter = Jira::new(&config, &Upstream::default()).await.unwrap();
    let data = adapter.discover().await.unwrap();
    assert!(data.issues.is_empty());
    assert!(data.issue_weblinks.is_empty());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.path() == "/rest/api/3/search/jql")
            .count(),
        2
    );
    assert!(
        !requests
            .iter()
            .any(|r| is_issue_history(r) || r.url.path() == "/rest/api/3/search")
    );
}

#[tokio::test]
async fn search_rejection_or_incomplete_pagination_preserves_last_good_snapshot_without_fallback() {
    for (failing_jql, failure) in [
        (ASSIGNED_JQL, "rejected"),
        (MENTION_JQL, "rejected"),
        (ASSIGNED_JQL, "page_failed"),
        (MENTION_JQL, "page_failed"),
        (MENTION_JQL, "missing_cursor"),
        (MENTION_JQL, "missing_terminal_flag"),
        (MENTION_JQL, "repeated_cursor"),
        (ASSIGNED_JQL, "page_bound"),
        (MENTION_JQL, "page_bound"),
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        for (jql, issues) in [
            (ASSIGNED_JQL, vec![upstream_issue("DEV-1", true, None)]),
            (MENTION_JQL, vec![]),
        ] {
            search_page(
                &server,
                jql,
                None,
                100,
                ResponseTemplate::new(200).set_body_json(json!({"issues":issues,"isLast":true})),
            )
            .await;
        }
        issue_history(&server, "DEV-1", vec![], false).await;
        let limits = Upstream {
            max_pages: if failure == "page_bound" { 1 } else { 100 },
            ..Default::default()
        };
        let mut jira_config = jira(&server.uri());
        // A fully empty configured group needs one page, leaving the search bound independent.
        jira_config.assignee_groups = vec!["empty".into()];
        mock_get(
            &server,
            "/rest/api/3/group/bulk",
            json!({"startAt":0,"isLast":true,"values":[{"name":"empty","groupId":"empty"}]}),
        )
        .await;
        mock_get(
            &server,
            "/rest/api/3/group/member",
            json!({"startAt":0,"isLast":true,"values":[]}),
        )
        .await;
        let adapter = Jira::new(&jira_config, &limits).await.unwrap();
        let mut config = config();
        config.jira = Some(jira_config);
        let cache = Cache::open(&config.server.database, &config).await.unwrap();
        cache
            .verify(adapter.identity().await.unwrap())
            .await
            .unwrap();
        let before = cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: adapter.discover().await,
            }])
            .await
            .unwrap();
        assert_eq!(before.board.issues.len(), 1);
        server.verify().await;
        server.reset().await;
        mock_jira(&server).await;
        mock_get(
            &server,
            "/rest/api/3/group/bulk",
            json!({"startAt":0,"isLast":true,"values":[{"name":"empty","groupId":"empty"}]}),
        )
        .await;
        mock_get(
            &server,
            "/rest/api/3/group/member",
            json!({"startAt":0,"isLast":true,"values":[]}),
        )
        .await;
        if failing_jql == MENTION_JQL {
            search_page(
                &server,
                ASSIGNED_JQL,
                None,
                100,
                ResponseTemplate::new(200).set_body_json(
                    json!({"issues":[upstream_issue("DEV-2",true,None)],"isLast":true}),
                ),
            )
            .await;
        }
        let candidate = upstream_issue("OTHER-3", false, Some(mentioned()));
        let first = match failure {
            "rejected" => ResponseTemplate::new(400).set_body_string("private upstream error"),
            "missing_cursor" => ResponseTemplate::new(200)
                .set_body_json(json!({"issues":[candidate],"isLast":false})),
            "missing_terminal_flag" => {
                ResponseTemplate::new(200).set_body_json(json!({"issues":[candidate]}))
            }
            _ => ResponseTemplate::new(200).set_body_json(
                json!({"issues":[candidate],"isLast":false,"nextPageToken":"page-2"}),
            ),
        };
        search_page(&server, failing_jql, None, 100, first).await;
        let mut expected_pages = 1 + usize::from(failing_jql == MENTION_JQL);
        if failure == "page_failed" {
            search_page(
                &server,
                failing_jql,
                Some("page-2"),
                100,
                ResponseTemplate::new(500),
            )
            .await;
            expected_pages += 1;
        } else if failure == "repeated_cursor" {
            // Detect a cycle of any length, not just a repeated adjacent token.
            for (cursor, next) in [("page-2", "page-3"), ("page-3", "page-2")] {
                search_page(
                    &server,
                    failing_jql,
                    Some(cursor),
                    100,
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"issues":[],"isLast":false,"nextPageToken":next})),
                )
                .await;
                expected_pages += 1;
            }
        }
        let error = adapter.discover().await.unwrap_err();
        assert!(!error.message.contains("private upstream error"));
        let after = cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: Err(error),
            }])
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&after.board.issues).unwrap(),
            serde_json::to_value(&before.board.issues).unwrap(),
            "{failure}"
        );
        assert_eq!(after.issue_weblinks, before.issue_weblinks);
        assert_eq!(
            after.board.sources[0].last_success_at,
            before.board.sources[0].last_success_at
        );
        assert_eq!(
            after.board.sources[0].coverage_notes,
            before.board.sources[0].coverage_notes
        );
        assert_eq!(after.board.sources[0].status, "error");
        assert!(after.board.sources[0].last_attempt_at >= before.board.sources[0].last_attempt_at);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.path() == "/rest/api/3/search/jql")
                .count(),
            expected_pages,
            "{failure}"
        );
        assert!(
            !requests
                .iter()
                .any(|r| is_issue_history(r) || r.url.path() == "/rest/api/3/search")
        );
    }
}

#[tokio::test]
async fn workflow_mappings_are_suggestions_not_an_allowlist_of_explicit_statuses() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let adapter = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    ready_jira(&adapter).await;
    let mut request = create();
    request.status = Some("QA validation".into());
    assert!(adapter.validate_create(&request, &[]).is_ok());
    let result = adapter.create(&request, &pr(), &[], true).await.unwrap();
    assert_eq!(result.outcome, "simulated");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
