mod common;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use common::*;
use pullboard::{
    api::{App, router},
    cache::{SourceData, SourceUpdate},
    model::Board,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    app: Arc<App>,
    method: &str,
    path: &str,
    body: Value,
    origin: Option<&str>,
    header: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if header {
        request = request.header("x-pullboard-request", "1");
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = router(app)
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn empty_safe_instance_contract_and_json_routes() {
    let app = App::build(config()).await.unwrap();
    for (path, name) in [
        ("/api/health", "Health"),
        ("/api/context", "Context"),
        ("/api/runtime-mode", "RuntimeMode"),
        ("/api/board", "Board"),
        ("/api/sync", "SyncStatus"),
    ] {
        let (s, v) = call(app.clone(), "GET", path, json!({}), None, false).await;
        assert_eq!(s, StatusCode::OK);
        validate_schema(&v, name);
    }
    let (s, v) = call(
        app.clone(),
        "GET",
        "/api/nonexistent",
        json!({}),
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    validate_schema(&v, "ErrorResponse");
    let (s, v) = call(app.clone(), "GET", "/api/", json!({}), None, false).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    validate_schema(&v, "ErrorResponse");
    let (s, _) = call(app, "POST", "/api/sync", json!({}), None, true).await;
    assert_eq!(s, StatusCode::ACCEPTED);
}
#[tokio::test]
async fn csrf_extraction_unknown_fields_and_nullable_required_fields() {
    let app = App::build(config()).await.unwrap();
    for (header, origin, expected) in [
        (false, None, StatusCode::FORBIDDEN),
        (true, Some("https://evil.example"), StatusCode::FORBIDDEN),
        (true, Some("null"), StatusCode::FORBIDDEN),
        (true, Some("http://localhost:8080"), StatusCode::ACCEPTED),
    ] {
        let (s, v) = call(app.clone(), "POST", "/api/sync", json!({}), origin, header).await;
        assert_eq!(s, expected);
        if s.is_client_error() {
            validate_schema(&v, "ErrorResponse");
        }
    }
    let (s, _) = call(
        app.clone(),
        "POST",
        "/api/sync",
        json!({"unknown":true}),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        app.clone(),
        "POST",
        "/api/ticket-drafts",
        json!({"pull_request_id":"x","purpose":"review"}),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        app,
        "GET",
        "/api/jira/options?unknown=true",
        json!({}),
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn malformed_and_oversize_body_are_uniform_json_errors() {
    let app = App::build(config()).await.unwrap();
    for body in ["{".into(), " ".repeat(pullboard::api::BODY_LIMIT + 1)] {
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sync")
                    .header("content-type", "application/json")
                    .header("x-pullboard-request", "1")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let data: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        validate_schema(&data, "ErrorResponse");
    }
}

#[tokio::test]
async fn host_header_blocks_dns_rebinding_reads() {
    let app = App::build(config()).await.unwrap();
    let response = router(app)
        .oneshot(
            Request::builder()
                .uri("/api/board")
                .header("Host", "attacker.example:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let data: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    validate_schema(&data, "ErrorResponse");
    assert_eq!(data["error"]["code"], "host_rejected");
}
#[tokio::test]
async fn simulated_create_link_and_draft_never_write_or_mutate_cache() {
    let server = wiremock::MockServer::start().await;
    mock_jira(&server).await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.forges = vec![forge("github", "https://api.github.com")];
    let app = App::build(c).await.unwrap();
    let j = app.jira.as_ref().unwrap();
    j.identity().await.unwrap();
    j.refresh_metadata().await.unwrap();
    verify_sources(&app.cache, &app.config).await;
    app.cache
        .publish(vec![
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    jira_choices: app.cache.read().await.unwrap().jira_choices,
                    issues: vec![issue()],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    let before = serde_json::to_value(app.cache.read().await.unwrap()).unwrap();
    let (s, v) = call(
        app.clone(),
        "POST",
        "/api/ticket-drafts",
        json!({"pull_request_id":pr().id,"purpose":"review","project_key":null}),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    validate_schema(&v, "TicketDraft");
    assert_eq!(v["generated_by"], "template");
    assert_eq!(v["title"], "Review widget/42: Improve caching DEV-1");
    let (s, v) = call(
        app.clone(),
        "POST",
        "/api/jira/issues",
        {
            let mut request = create();
            request.expected_mode_revision = app.mode.read().mode_revision;
            serde_json::to_value(request).unwrap()
        },
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    validate_schema(&v, "CreateIssueResult");
    assert_eq!(v["outcome"], "simulated");
    assert!(v["issue"].is_null());
    let (s, v) = call(
        app.clone(),
        "POST",
        "/api/jira/issues/DEV-1/links",
        json!({"pull_request_id":pr().id,"expected_mode_revision":app.mode.read().mode_revision}),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["outcome"], "simulated");
    validate_schema(&v, "LinkResult");
    let after = serde_json::to_value(app.cache.read().await.unwrap()).unwrap();
    assert_eq!(before, after);
    assert_eq!(app.sync.status().await.unwrap().state, "idle");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
    let (s, _) = call(
        app.clone(),
        "POST",
        "/api/jira/issues/DEV-999/links",
        json!({"pull_request_id":pr().id,"expected_mode_revision":app.mode.read().mode_revision}),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let board: Board = app.cache.read().await.unwrap().board;
    validate_schema(&serde_json::to_value(board).unwrap(), "Board");
}

#[tokio::test]
async fn runtime_mode_is_shared_atomic_guarded_and_resets_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("mode.sqlite");
    let app = App::build(c.clone()).await.unwrap();
    let original = app.mode.read();
    let (s, no_op) = call(
        app.clone(),
        "POST",
        "/api/runtime-mode",
        json!({
            "read_only_test_mode":true,"expected_mode_revision":original.mode_revision
        }),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(no_op, serde_json::to_value(&original).unwrap());
    let change =
        json!({"read_only_test_mode":false,"expected_mode_revision":original.mode_revision});
    let (a, b) = tokio::join!(
        call(
            app.clone(),
            "POST",
            "/api/runtime-mode",
            change.clone(),
            None,
            true
        ),
        call(app.clone(), "POST", "/api/runtime-mode", change, None, true)
    );
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert!([a.0, b.0].contains(&StatusCode::CONFLICT));
    let live = app.mode.read();
    assert!(!live.read_only_test_mode);
    assert_ne!(original.mode_revision, live.mode_revision);
    for (path, schema) in [
        ("/api/health", "Health"),
        ("/api/context", "Context"),
        ("/api/runtime-mode", "RuntimeMode"),
    ] {
        let (s, value) = call(app.clone(), "GET", path, json!({}), None, false).await;
        assert_eq!(s, StatusCode::OK);
        validate_schema(&value, schema);
        assert_eq!(value["read_only_test_mode"], false);
        assert_eq!(value["mode_revision"], live.mode_revision);
    }
    // Toggling back cannot revive an old dialog with the same boolean.
    let (_, back) = call(
        app.clone(),
        "POST",
        "/api/runtime-mode",
        json!({
            "read_only_test_mode":true,"expected_mode_revision":live.mode_revision
        }),
        None,
        true,
    )
    .await;
    assert_ne!(back["mode_revision"], original.mode_revision);
    app.mode
        .change(back["mode_revision"].as_str().unwrap(), false)
        .unwrap();
    assert_eq!(app.sync.status().await.unwrap().state, "idle");
    assert_eq!(app.cache.read().await.unwrap().board.revision, 0);
    drop(app);
    let restarted = App::build(c).await.unwrap();
    assert!(restarted.mode.read().read_only_test_mode);
    assert_ne!(restarted.mode.read().mode_revision, original.mode_revision);
    assert_ne!(restarted.mode.read().mode_revision, back["mode_revision"]);
    let (s, v) = call(
        restarted,
        "POST",
        "/api/runtime-mode",
        json!({
            "read_only_test_mode":false,"expected_mode_revision":original.mode_revision
        }),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "mode_conflict");
    assert!(v.get("mode_revision").is_none());
}

#[tokio::test]
async fn malformed_revisions_are_400_but_stale_actions_conflict_before_any_reads_or_business_validation()
 {
    let server = wiremock::MockServer::start().await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    let app = App::build(c).await.unwrap();
    for path in [
        "/api/runtime-mode",
        "/api/jira/issues",
        "/api/jira/issues/DEV-1/links",
    ] {
        let body = if path == "/api/runtime-mode" {
            json!({"read_only_test_mode":false})
        } else if path.ends_with("links") {
            json!({"pull_request_id":""})
        } else {
            serde_json::to_value(create()).unwrap()
        };
        for invalid in [
            Value::Null,
            json!(false),
            json!(17),
            json!(""),
            json!("x".repeat(129)),
        ] {
            let mut request = body.clone();
            request["expected_mode_revision"] = invalid;
            let (s, v) = call(app.clone(), "POST", path, request, None, true).await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
            assert_eq!(v["error"]["code"], "invalid_request");
        }
        let mut missing = body.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("expected_mode_revision");
        assert_eq!(
            call(app.clone(), "POST", path, missing, None, true).await.0,
            StatusCode::BAD_REQUEST
        );
        let mut stale = body;
        stale["expected_mode_revision"] = json!("well-formed-but-unknown");
        // All cached IDs/project/business choices may be wrong: conflict wins.
        let (s, v) = call(app.clone(), "POST", path, stale.clone(), None, true).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "mode_conflict");
        validate_schema(&v, "ErrorResponse");
        stale["unknown"] = json!(true);
        assert_eq!(
            call(app.clone(), "POST", path, stale, None, true).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(app.cache.read().await.unwrap().board.revision, 0);
    assert_eq!(app.sync.status().await.unwrap().state, "idle");
    for (header, origin) in [(false, None), (true, Some("https://evil.example"))] {
        let (s, v) = call(
            app.clone(),
            "POST",
            "/api/runtime-mode",
            json!({
                "read_only_test_mode":false,"expected_mode_revision":app.mode.read().mode_revision
            }),
            origin,
            header,
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        validate_schema(&v, "ErrorResponse");
        assert!(app.mode.read().read_only_test_mode);
    }
}

#[tokio::test]
async fn admitted_action_keeps_mode_through_validation_and_all_live_followups() {
    use wiremock::{Mock, ResponseTemplate, matchers::*};
    for simulated in [true, false] {
        let server = wiremock::MockServer::start().await;
        mock_jira(&server).await;
        mock_get(&server, "/rest/agile/1.0/board/12/sprint", json!({
            "startAt":0,"maxResults":100,"isLast":true,"values":[{"id":123,"name":"2026-Platform-S1","state":"active"}]
        })).await;
        Mock::given(method("GET")).and(path("/rest/agile/1.0/board/12/sprint"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "startAt":0,"maxResults":100,"isLast":true,"values":[{"id":123,"name":"2026-Platform-S1","state":"active"}]
            }))).with_priority(1).mount(&server).await;
        let mut c = config();
        c.read_only_test_mode = simulated;
        c.jira = Some(jira(&server.uri()));
        c.forges = vec![forge("github", "https://api.github.com")];
        let app = App::build(c).await.unwrap();
        verify_sources(&app.cache, &app.config).await;
        app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
        app.cache
            .publish(vec![SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            }])
            .await
            .unwrap();
        let before = serde_json::to_value(app.cache.read().await.unwrap()).unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let notify = entered.clone();
        Mock::given(method("GET"))
            .and(path("/rest/api/3/myself"))
            .respond_with(move |_: &wiremock::Request| {
                notify.notify_one();
                ResponseTemplate::new(200)
                    .set_body_json(jira_user("me"))
                    .set_delay(std::time::Duration::from_millis(150))
            })
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-88"})))
            .expect(u64::from(!simulated))
            .mount(&server)
            .await;
        for path in [
            "/rest/agile/1.0/sprint/123/issue",
            "/rest/api/3/issue/DEV-88/remotelink",
            "/rest/api/3/issue/DEV-88/transitions",
        ] {
            Mock::given(method("POST"))
                .and(wiremock::matchers::path(path))
                .respond_with(ResponseTemplate::new(204))
                .expect(u64::from(!simulated))
                .mount(&server)
                .await;
        }
        mock_get(
            &server,
            "/rest/api/3/issue/DEV-88/transitions",
            json!({"transitions":[{"id":"7","name":"Review","to":{"id":"20","name":"Review"}}]}),
        )
        .await;
        mock_get(
            &server,
            "/rest/api/3/issue/DEV-88",
            json!({"fields":{"status":{"id":"10","name":"Open"}}}),
        )
        .await;
        let mut request = create();
        request.expected_mode_revision = app.mode.read().mode_revision;
        request.sprint_id = Some("123".into());
        request.status = Some("Review".into());
        let action_app = app.clone();
        let action = tokio::spawn(async move {
            call(
                action_app,
                "POST",
                "/api/jira/issues",
                serde_json::to_value(request).unwrap(),
                None,
                true,
            )
            .await
        });
        // Simulated validation now performs no upstream I/O. Live identity checking
        // still provides an admission boundary to toggle during an in-flight action.
        if !simulated {
            tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
                .await
                .unwrap();
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while !action.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        let (s, _) = call(app.clone(), "POST", "/api/runtime-mode", json!({
            "read_only_test_mode":!simulated,"expected_mode_revision":app.mode.read().mode_revision
        }), None, true).await;
        assert_eq!(s, StatusCode::OK);
        let (s, response) = action.await.unwrap();
        assert_eq!(s, StatusCode::OK, "{response}");
        validate_schema(&response, "CreateIssueResult");
        assert_eq!(
            response["outcome"],
            if simulated { "simulated" } else { "created" }
        );
        assert_eq!(response["warnings"], json!([]));
        assert_eq!(app.mode.read().read_only_test_mode, !simulated);
        assert_eq!(app.config.read_only_test_mode, simulated);
        let writes = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method != "GET")
            .count();
        assert_eq!(writes, if simulated { 0 } else { 4 });
        assert_eq!(
            before,
            serde_json::to_value(app.cache.read().await.unwrap()).unwrap()
        );
        assert_eq!(
            app.sync.status().await.unwrap().state,
            if simulated { "idle" } else { "syncing" }
        );
    }
}

#[tokio::test]
async fn admitted_live_link_finishes_after_switching_to_test_and_old_tokens_cannot_link_again() {
    use wiremock::{Mock, ResponseTemplate, matchers::*};
    let server = wiremock::MockServer::start().await;
    let mut c = config();
    c.read_only_test_mode = false;
    c.jira = Some(jira(&server.uri()));
    c.forges = vec![forge("github", "https://api.github.com")];
    let app = App::build(c).await.unwrap();
    verify_sources(&app.cache, &app.config).await;
    let mut unlinked = issue();
    unlinked.key = "DEV-2".into();
    app.cache
        .publish(vec![
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![unlinked],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    let old = app.mode.read();
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(move |_: &wiremock::Request| {
            notify.notify_one();
            ResponseTemplate::new(200)
                .set_body_json(jira_user("me"))
                .set_delay(std::time::Duration::from_millis(150))
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/DEV-2/remotelink"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let action_app = app.clone();
    let token = old.mode_revision.clone();
    let action = tokio::spawn(async move {
        call(
            action_app,
            "POST",
            "/api/jira/issues/DEV-2/links",
            json!({
                "pull_request_id":pr().id,"expected_mode_revision":token
            }),
            None,
            true,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    app.mode.change(&old.mode_revision, true).unwrap();
    let (s, response) = action.await.unwrap();
    assert_eq!(s, StatusCode::OK);
    assert_eq!(response["outcome"], "linked");
    validate_schema(&response, "LinkResult");
    assert!(app.mode.read().read_only_test_mode);
    let before = server.received_requests().await.unwrap().len();
    let (s, response) = call(
        app.clone(),
        "POST",
        "/api/jira/issues/DEV-2/links",
        json!({
            "pull_request_id":pr().id,"expected_mode_revision":old.mode_revision
        }),
        None,
        true,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(response["error"]["code"], "mode_conflict");
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(app.sync.status().await.unwrap().state, "syncing");
}

#[tokio::test]
async fn purpose_type_options_and_context_match_frozen_discovered_contract_and_pending_errors() {
    let server = wiremock::MockServer::start().await;
    mock_jira(&server).await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    let app = App::build(c).await.unwrap();
    let (_, context) = call(app.clone(), "GET", "/api/context", json!({}), None, false).await;
    assert_eq!(context["identities"], json!([]));
    assert_eq!(context["jira_projects"], json!([]));
    for path in [
        "/api/jira/options?project_key=DEV",
        "/api/jira/options?project_key=DEV&purpose=bad",
        "/api/jira/options?project_key=&purpose=review",
        "/api/jira/options?project_key=DEV&purpose=review&issue_type=",
        "/api/jira/options?project_key=DEV&purpose=review&groups=admin",
    ] {
        let (s, v) = call(app.clone(), "GET", path, json!({}), None, false).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        validate_schema(&v, "ErrorResponse");
    }
    let (s, v) = call(
        app.clone(),
        "GET",
        "/api/jira/options?project_key=DEV&purpose=review",
        json!({}),
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"]["code"], "jira_not_ready");
    let j = app.jira.as_ref().unwrap();
    let identity = j.identity().await.unwrap();
    app.cache.verify(identity).await.unwrap();
    j.refresh_metadata().await.unwrap();
    let (s, v) = call(
        app.clone(),
        "GET",
        "/api/jira/options?project_key=DEV&purpose=implementation&issue_type=Story&query=nobody",
        json!({}),
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    validate_schema(&v, "JiraOptions");
    assert_eq!(v["issue_type"], "Story");
    assert_eq!(v["defaults"]["issue_type"], "Task");
    assert!(v["defaults"]["assignee_id"].is_null());
    assert_eq!(v["assignees"], json!([]));
    let (s, v) = call(
        app.clone(),
        "GET",
        "/api/jira/options?project_key=DEV&purpose=review",
        json!({}),
        None,
        false,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    validate_schema(&v, "JiraOptions");
    assert!(v["defaults"]["assignee_id"].is_null());
    assert!(
        v["assignees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != "me")
    );
    let (_, v) = call(app.clone(), "GET", "/api/context", json!({}), None, false).await;
    validate_schema(&v, "Context");
    assert_eq!(v["identities"][0]["account_id"], "me");
    assert_eq!(v["jira_projects"][0]["name"], "Development");
    assert!(v["jira_projects"][0].get("default_issue_type").is_none());
}
#[tokio::test]
async fn spa_fallback_does_not_capture_api_and_method_errors_are_json() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("index.html"),
        "<html>test Angular build</html>",
    )
    .await
    .unwrap();
    let mut c = config();
    c.server.static_directory = dir.path().into();
    let app = App::build(c).await.unwrap();
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/nested/client/route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    let (s, v) = call(app, "DELETE", "/api/health", json!({}), None, false).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    validate_schema(&v, "ErrorResponse");
}

// Validate against the shared JSON Schema with a standard validator. HTTP/file
// resolution features are disabled: the wire contract must be self-contained.
fn validate_schema(value: &Value, name: &str) {
    let doc: Value = serde_json::from_str(include_str!("../../api/openapi.json")).unwrap();
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": format!("#/components/schemas/{name}"),
        "components": doc["components"]
    });
    let validator = jsonschema::validator_for(&schema).unwrap();
    if let Err(error) = validator.validate(value) {
        panic!("{name} violates the API contract: {error}");
    }
}
