mod common;
use common::*;
use pullboard::{config::Upstream, jira::Jira, model::Purpose};
use serde_json::json;
use std::sync::Arc;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

async fn boards(server: &MockServer) {
    // Primary board is numerically greater. Shared IDs must nevertheless retain
    // its name/state/URL; foreign originBoardId must never cause another read.
    for (board, rows) in [
        (
            12,
            vec![
                (101, "SSSD-IDENTITY", "active"),
                (102, "SSSD-CORE", "active"),
                (103, "UAH", "future"),
            ],
        ),
        (
            8,
            vec![
                (101, "secondary duplicate", "future"),
                (104, "SSSD-IDENTITY secondary", "active"),
                (105, "SSSD-CORE future", "future"),
            ],
        ),
    ] {
        for (at, (id, name, state)) in rows.into_iter().enumerate() {
            Mock::given(method("GET"))
                .and(path(format!("/rest/agile/1.0/board/{board}/sprint")))
                .and(query_param("startAt", at.to_string()))
                .and(query_param("state", "active,future"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "startAt":at,"maxResults":1,"isLast":at==2,
                    "values":[{"id":id,"name":name,"state":state,"originBoardId":999}]
                })))
                .with_priority(2)
                .mount(server)
                .await;
        }
    }
}

#[tokio::test]
async fn full_multi_board_pagination_deduplicates_with_configured_provenance_and_no_fallback() {
    let server = MockServer::start().await;
    boards(&server).await;
    let mut config = jira(&server.uri());
    config.board_ids = vec![12, 8];
    config.sprint_name_patterns = vec![
        "SSSD-IDENTITY".parse().unwrap(),
        "SSSD-CORE".parse().unwrap(),
    ];
    let j = Jira::new(
        &config,
        &Upstream {
            page_size: 1,
            max_concurrent_requests: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let sprints = j.sprints().await.unwrap();
    assert_eq!(sprints.len(), 5);
    let shared = sprints.iter().find(|s| s.id == "101").unwrap();
    assert_eq!(shared.name, "SSSD-IDENTITY");
    assert_eq!(shared.board_id, 12);
    assert!(shared.board_url.ends_with("rapidView=12"));
    assert!(shared.default_eligible);
    assert_eq!(sprints.iter().filter(|s| s.default_eligible).count(), 1);
    assert!(sprints.iter().any(|s| s.name == "UAH"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 6);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "GET" && !r.url.path().contains("999"))
    );

    config.sprint_name_patterns = vec![
        "secondary".parse().unwrap(),
        "SSSD-IDENTITY".parse().unwrap(),
    ];
    let j = Jira::new(&config, &Upstream::default()).await.unwrap();
    assert!(
        j.sprints()
            .await
            .unwrap()
            .iter()
            .all(|s| !s.default_eligible)
    );
}

#[tokio::test]
async fn repeated_sprints_within_one_board_and_unexpected_states_fail_the_complete_union() {
    for invalid_state in [None, Some("closed")] {
        let server = MockServer::start().await;
        let mut config = jira(&server.uri());
        config.board_ids = vec![12, 8];
        Mock::given(method("GET"))
            .and(path("/rest/agile/1.0/board/12/sprint"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "values":[{"id":101,"name":"Identity","state":"active"}],
                "startAt":0,"maxResults":1,"isLast":true
            })))
            .mount(&server)
            .await;
        for at in 0..=1 {
            Mock::given(method("GET"))
                .and(path("/rest/agile/1.0/board/8/sprint"))
                .and(query_param("startAt", at.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "values":[{"id":102,"name":"CORE","state":invalid_state.unwrap_or("future")}],
                    "startAt":at,"maxResults":1,"isLast":at==1
                })))
                .mount(&server)
                .await;
        }
        let j = Jira::new(&config, &Upstream::default()).await.unwrap();
        assert_eq!(j.sprints().await.unwrap_err().code, "jira_discovery_failed");
    }
}

#[tokio::test]
async fn committed_patterns_filter_options_and_submission_and_survive_new_cards_and_failed_options()
{
    let server = MockServer::start().await;
    mock_jira(&server).await;
    boards(&server).await;
    let mut config = jira(&server.uri());
    config.board_ids = vec![12, 8];
    config.sprint_name_patterns = vec![
        "SSSD-IDENTITY".parse().unwrap(),
        "SSSD-CORE".parse().unwrap(),
    ];
    let mut j = Jira::new(&config, &Upstream::default()).await.unwrap();
    let cache = bind_jira(&j).await;
    j.refresh_metadata().await.unwrap();
    let initial = cache.read().await.unwrap();
    assert_eq!(initial.board.sprints.len(), 5);
    assert_eq!(initial.jira_choices.as_ref().unwrap().sprints.len(), 5);
    let options = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert_eq!(
        options
            .sprints
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["101", "102", "104", "105"]
    );
    assert!(options.defaults.sprint_id.is_none());
    let mut request = create();
    request.sprint_id = Some("102".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap().outcome,
        "simulated"
    );
    request.sprint_id = Some("103".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );

    // A policy-only change must not reinterpret the committed generation, even
    // when the card domain successfully publishes a newer complete sprint union.
    Arc::get_mut(&mut j).unwrap().config.sprint_name_patterns = vec!["UAH".parse().unwrap()];
    Mock::given(method("GET"))
        .and(path("/rest/api/3/project/DEV/statuses"))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(j.refresh_metadata().await.is_err());
    let failed = cache.read().await.unwrap();
    assert_eq!(failed.board.sources[0].status, "ok");
    assert_eq!(failed.jira_options_health.as_ref().unwrap().status, "error");
    assert_eq!(
        failed.jira_options_health.as_ref().unwrap().last_success_at,
        initial
            .jira_options_health
            .as_ref()
            .unwrap()
            .last_success_at
    );
    assert!(
        !failed
            .board
            .sprints
            .iter()
            .find(|s| s.id == "101")
            .unwrap()
            .default_eligible
    );
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap()
            .sprints
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        options
            .sprints
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
    );
    request.sprint_id = Some("102".into());
    j.create(&request, &pr(), &[], true).await.unwrap();
    request.sprint_id = Some("103".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );

    // A required secondary-board page failure retains both complete domains.
    Mock::given(method("GET"))
        .and(path("/rest/agile/1.0/board/8/sprint"))
        .and(query_param("startAt", "2"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&server)
        .await;
    assert!(j.refresh_metadata().await.is_err());
    let failed = cache.read().await.unwrap();
    assert_eq!(failed.board.sources[0].status, "error");
    assert_eq!(failed.board.sprints.len(), 5);
    assert_eq!(failed.jira_choices.unwrap().sprints.len(), 5);
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap()
            .sprints
            .len(),
        4
    );
    // Only a complete successful replacement applies the new regex policy.
    server.reset().await;
    mock_jira(&server).await;
    boards(&server).await;
    j.refresh_metadata().await.unwrap();
    let replaced = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert_eq!(
        replaced
            .sprints
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["103"]
    );
    request.sprint_id = Some("103".into());
    j.create(&request, &pr(), &[], true).await.unwrap();
    request.sprint_id = Some("102".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
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
