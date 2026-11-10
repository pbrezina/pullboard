mod common;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use common::*;
use pullboard::{
    api::{App, router},
    cache::{SourceData, SourceUpdate},
    model::*,
    sync::Syncer,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

async fn override_get(server: &MockServer, p: &str, value: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .with_priority(1)
        .mount(server)
        .await;
}
async fn fail_get(server: &MockServer, p: &str) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(403).set_body_string("private upstream diagnostic"))
        .with_priority(1)
        .mount(server)
        .await;
}
async fn call(app: &Arc<App>, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("x-pullboard-request", "1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let value = serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
        .unwrap();
    (status, value)
}
fn board_issue() -> UpstreamIssue {
    serde_json::from_value(json!({"id":"test-forge:acme/widget:issue:7","source_id":"test-forge","forge":"github","number":7,"owner":"acme","repository":"widget","title":"Clone this","url":"https://github.com/acme/widget/issues/7","description":"Exact text","state":"open","reporter":null,"assignees":[person("1")],"labels":[],"is_assigned_to_me":true,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"})).unwrap()
}
async fn editor_fixture(server: &MockServer) -> Arc<App> {
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.jira.as_mut().unwrap().defaults = serde_yaml::from_str("implementation: {issue_type: Task, assignee: me, workflow: {draft: Open, ready: Review}}\nreview: {issue_type: Task, assignee: null, workflow: {draft: Open, ready: Review}}").unwrap();
    c.forges = vec![forge("github", "https://api.github.com")];
    let app = App::build(c).await.unwrap();
    app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
    app.cache
        .verify(identity("test-forge", Kind::Github, "1"))
        .await
        .unwrap();
    app.cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                pull_requests: vec![pr()],
                upstream_issues: vec![board_issue()],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    app
}
fn clone_request(app: &App) -> Value {
    json!({"upstream_issue_id":board_issue().id,"expected_mode_revision":app.mode.read().mode_revision,"purpose":"implementation","project_key":"DEV","issue_type":"Task","title":"Clone","description":"Text","story_points":null,"assignee_id":null,"sprint_id":null,"team":null,"status":null})
}

#[tokio::test]
async fn options_email_search_both_drafts_and_both_simulated_creates_make_zero_discovery_requests()
{
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut exposed = jira_user("other");
    exposed["displayName"] = json!("Visible name");
    exposed["emailAddress"] = json!("exposed-address@jira.invalid");
    // Override only the matching account, rather than an upstream arbitrary-user search.
    Mock::given(method("GET"))
        .and(path("/rest/api/3/user/assignable/search"))
        .and(query_param("accountId", "other"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([exposed])))
        .with_priority(1)
        .mount(&server)
        .await;
    let app = editor_fixture(&server).await;
    let log = server.received_requests().await.unwrap();
    for endpoint in [
        "/rest/api/3/field",
        "/rest/api/3/project/DEV/statuses",
        "/rest/agile/1.0/board/12/sprint",
    ] {
        assert_eq!(
            log.iter().filter(|r| r.url.path() == endpoint).count(),
            1,
            "{endpoint}"
        );
    }
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
            .count(),
        2
    );
    let requests = log.len();
    let before = serde_json::to_value(app.cache.read().await.unwrap()).unwrap();
    for query in [
        "",
        "&query=EXPOSED-ADDRESS",
        "&query=unmatched",
        "&issue_type=Story",
    ] {
        let (status, options) = call(
            &app,
            "GET",
            &format!("/api/jira/options?project_key=DEV&purpose=review{query}"),
            json!({}),
        )
        .await;
        assert_eq!(status, 200, "{options}");
        validate_contract(&options, "JiraOptions");
        assert!(options["defaults"]["assignee_id"].is_null());
        assert!(options["defaults"]["sprint_id"].is_null());
        assert_eq!(options["diagnostics"], json!([]));
        if query == "&query=EXPOSED-ADDRESS" {
            assert_eq!(
                options["assignees"][0]["email"],
                "exposed-address@jira.invalid"
            );
        }
        if query == "&query=unmatched" {
            assert_eq!(options["assignees"], json!([]));
        }
        if query == "&issue_type=Story" {
            assert_eq!(
                options["statuses"],
                json!([{"id":"40","name":"In progress"},{"id":"10","name":"Open"}])
            );
        }
    }
    for (endpoint, request, schema) in [
        (
            "/api/ticket-drafts",
            json!({"pull_request_id":pr().id,"purpose":"implementation","project_key":null}),
            "TicketDraft",
        ),
        (
            "/api/upstream-issues/ticket-drafts",
            json!({"upstream_issue_id":board_issue().id,"project_key":null}),
            "UpstreamIssueDraft",
        ),
    ] {
        let (status, draft) = call(&app, "POST", endpoint, request).await;
        assert_eq!(status, 200, "{draft}");
        validate_contract(&draft, schema);
        for field in ["assignee_id", "sprint_id", "suggested_status"] {
            assert!(draft[field].is_null(), "{draft}");
        }
        assert_eq!(draft["diagnostics"], json!([]));
    }
    let mut request = create();
    request.expected_mode_revision = app.mode.read().mode_revision;
    for (endpoint, request) in [
        ("/api/jira/issues", serde_json::to_value(request).unwrap()),
        ("/api/upstream-issues/jira-issues", clone_request(&app)),
    ] {
        let (status, result) = call(&app, "POST", endpoint, request).await;
        assert_eq!(status, 200, "{result}");
        assert_eq!(result["outcome"], "simulated");
        assert!(result["issue"].is_null());
    }
    assert_eq!(server.received_requests().await.unwrap().len(), requests);
    assert_eq!(
        serde_json::to_value(app.cache.read().await.unwrap()).unwrap(),
        before
    );
    assert_eq!(app.sync.status().await.unwrap().state, "idle");
    // Live creation may verify identity and perform explicit writes, but must not
    // repeat choice discovery either, for either selector kind.
    app.mode
        .change(&app.mode.read().mode_revision, false)
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-77"})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/DEV-77/remotelink"))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&server)
        .await;
    let mut request = create();
    request.expected_mode_revision = app.mode.read().mode_revision;
    for (endpoint, request) in [
        ("/api/jira/issues", serde_json::to_value(request).unwrap()),
        ("/api/upstream-issues/jira-issues", clone_request(&app)),
    ] {
        let (status, result) = call(&app, "POST", endpoint, request).await;
        assert_eq!(status, 200, "{result}");
        assert_eq!(result["outcome"], "created");
    }
    let log = server.received_requests().await.unwrap();
    assert!(
        log.iter()
            .skip(requests)
            .all(|r| r.url.path() == "/rest/api/3/myself" || r.method == "POST")
    );
}

#[tokio::test]
async fn initial_readiness_failed_discovery_and_genuinely_empty_pools_are_distinct_and_reads_do_not_retry()
 {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.forges = vec![forge("github", "https://api.github.com")];
    let app = App::build(c).await.unwrap();
    app.cache
        .verify(identity("test-forge", Kind::Github, "1"))
        .await
        .unwrap();
    app.cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                pull_requests: vec![pr()],
                upstream_issues: vec![board_issue()],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    let j = app.jira.as_ref().unwrap();
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    let mut request = create();
    request.expected_mode_revision = app.mode.read().mode_revision;
    for (endpoint, request) in [
        (
            "/api/ticket-drafts",
            json!({"pull_request_id":pr().id,"purpose":"review","project_key":null}),
        ),
        (
            "/api/upstream-issues/ticket-drafts",
            json!({"upstream_issue_id":board_issue().id,"project_key":null}),
        ),
        ("/api/jira/issues", serde_json::to_value(request).unwrap()),
        ("/api/upstream-issues/jira-issues", clone_request(&app)),
    ] {
        let (status, value) = call(&app, "POST", endpoint, request).await;
        assert_eq!(status, 503, "{endpoint}: {value}");
        assert_eq!(value["error"]["code"], "jira_not_ready");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    j.identity().await.unwrap();
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    fail_get(&server, "/rest/api/3/user/assignable/search").await;
    assert_eq!(
        j.refresh_metadata().await.unwrap_err().code,
        "jira_discovery_failed"
    );
    assert!(app.cache.read().await.unwrap().jira_choices.is_none());
    assert!(j.projects().await.is_empty());
    let count = server.received_requests().await.unwrap().len();
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_discovery_failed"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), count);
    server.reset().await;
    mock_jira(&server).await;
    override_get(&server, "/rest/api/3/users/search", json!([])).await;
    j.refresh_metadata().await.unwrap();
    assert!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap()
            .assignees
            .is_empty()
    );
    assert_eq!(
        app.cache.read().await.unwrap().board.sources[0].status,
        "ok"
    );
}

#[tokio::test]
async fn metadata_assignee_and_history_failures_retain_the_entire_last_good_generation() {
    for endpoint in [
        "/rest/api/3/field",
        "/rest/api/3/project/DEV/statuses",
        "/rest/api/3/user/assignable/search",
        "/rest/api/3/issue/DEV-1/comment",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let app = editor_fixture(&server).await;
        let j = app.jira.as_ref().unwrap();
        let before = app.cache.read().await.unwrap();
        // New project name is read successfully before a later mandatory failure.
        override_get(
            &server,
            "/rest/api/3/project/DEV",
            json!({"key":"DEV","name":"Uncommitted name"}),
        )
        .await;
        override_get(&server, "/rest/api/3/search/jql", json!({"isLast":true,"issues":[{"id":"1","key":"DEV-1","fields":{"summary":"New issue","project":{"key":"DEV"},"issuetype":{"name":"Task"},"status":{"name":"Open"},"assignee":jira_user("me"),"reporter":null,"created":"2026-01-01T00:00:00Z","updated":"2026-01-02T00:00:00Z"}}]})).await;
        mock_get(&server, "/rest/api/3/issue/DEV-1/remotelink", json!([])).await;
        fail_get(&server, endpoint).await;
        assert!(j.refresh_metadata().await.is_err(), "{endpoint}");
        let after = app.cache.read().await.unwrap();
        assert_eq!(
            serde_json::to_value(&after.jira_choices).unwrap(),
            serde_json::to_value(&before.jira_choices).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.board.issues).unwrap(),
            serde_json::to_value(&before.board.issues).unwrap()
        );
        assert_eq!(
            after.board.sources[0].last_success_at,
            before.board.sources[0].last_success_at
        );
        assert_eq!(after.board.sources[0].status, "error");
        assert!(
            !after.board.sources[0]
                .error
                .as_ref()
                .unwrap()
                .contains("private")
        );
        let count = server.received_requests().await.unwrap().len();
        assert_eq!(j.projects().await[0].name, "Development");
        assert_eq!(
            j.options("DEV", Purpose::Review, None, "")
                .await
                .unwrap()
                .assignees
                .len(),
            1
        );
        assert_eq!(
            j.create(&create(), &pr(), &[], true).await.unwrap().outcome,
            "simulated"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), count);
    }
}

async fn wait_idle(sync: &Syncer) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if sync.status().await.unwrap().state == "idle" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_atomic_commit_never_publishes_choices_or_success_and_the_next_commit_switches_both()
{
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.server.database = dir.path().join("choices.sqlite");
    let app = App::build(c).await.unwrap();
    app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
    let before = app.cache.read().await.unwrap();
    let pool = sqlx::SqlitePool::connect(app.config.server.database.to_str().unwrap())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_choice_publication BEFORE UPDATE ON pullboard_snapshot_v1 WHEN json_extract(NEW.snapshot,'$.jira_choices.projects[0].name') = 'New generation' BEGIN SELECT RAISE(FAIL,'injected publication failure'); END").execute(&pool).await.unwrap();
    override_get(
        &server,
        "/rest/api/3/project/DEV",
        json!({"key":"DEV","name":"New generation"}),
    )
    .await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    app.sync.request().await.unwrap();
    wait_idle(&app.sync).await;
    let after = app.cache.read().await.unwrap();
    assert_eq!(
        serde_json::to_value(&after.jira_choices).unwrap(),
        serde_json::to_value(&before.jira_choices).unwrap()
    );
    assert_eq!(after.board.revision, before.board.revision);
    assert_eq!(after.board.generated_at, before.board.generated_at);
    assert_eq!(
        after.board.sources[0].last_success_at,
        before.board.sources[0].last_success_at
    );
    let live = app.sync.status().await.unwrap();
    assert_eq!(live.sources[0].status, "error");
    assert_eq!(live.progress[0].phase, SourcePhase::Failed);
    assert_eq!(
        app.jira.as_ref().unwrap().projects().await[0].name,
        "Development"
    );
    sqlx::query("DROP TRIGGER reject_choice_publication")
        .execute(&pool)
        .await
        .unwrap();
    app.sync.request().await.unwrap();
    wait_idle(&app.sync).await;
    let after = app.cache.read().await.unwrap();
    assert_eq!(
        after.jira_choices.unwrap().projects[0].name,
        "New generation"
    );
    assert_eq!(after.board.sources[0].status, "ok");
    assert!(after.board.sources[0].last_success_at > before.board.sources[0].last_success_at);
    cancel.cancel();
    worker.await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn principal_change_quarantines_cached_choices_before_a_failed_refresh() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let app = editor_fixture(&server).await;
    let j = app.jira.as_ref().unwrap();
    override_get(
        &server,
        "/rest/api/3/myself",
        jira_user("another-principal"),
    )
    .await;
    j.identity().await.unwrap();
    assert!(app.cache.read().await.unwrap().jira_choices.is_none());
    assert!(j.projects().await.is_empty());
    assert_eq!(
        j.options("DEV", Purpose::Review, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    fail_get(&server, "/rest/api/3/field").await;
    assert!(j.refresh_metadata().await.is_err());
    assert_eq!(
        j.options("DEV", Purpose::Review, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_discovery_failed"
    );
    let snapshot = app.cache.read().await.unwrap();
    assert_eq!(
        snapshot
            .identities
            .iter()
            .find(|i| i.source_id == "jira")
            .unwrap()
            .account_id,
        "another-principal"
    );
    assert!(snapshot.board.sources[0].last_success_at.is_none());
}

#[tokio::test]
async fn changed_group_and_default_policy_are_committed_together_not_applied_to_a_restored_pool() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.server.database = dir.path().join("policy.sqlite");
    let app = App::build(c.clone()).await.unwrap();
    app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
    drop(app);
    let j = c.jira.as_mut().unwrap();
    j.assignee_groups = vec!["new-policy".into()];
    j.team = "Independent".into();
    j.defaults =
        serde_yaml::from_str("implementation: {issue_type: Story, assignee: null}").unwrap();
    let app = App::build(c).await.unwrap();
    let j = app.jira.as_ref().unwrap();
    j.identity().await.unwrap();
    let old = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert_eq!(old.issue_type.as_deref(), Some("Task"));
    assert_eq!(old.assignees.len(), 2);
    assert_eq!(
        old.teams
            .iter()
            .find(|t| Some(&t.id) == old.defaults.team.as_ref())
            .unwrap()
            .name,
        "Platform"
    );
    assert!(j.refresh_metadata().await.is_err()); // No successful new group discovery.
    assert_eq!(
        j.options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap()
            .assignees
            .len(),
        2
    );
    mock_get(
        &server,
        "/rest/api/3/group/bulk",
        json!({"startAt":0,"isLast":true,"values":[{"name":"new-policy","groupId":"real-group"}]}),
    )
    .await;
    mock_get(
        &server,
        "/rest/api/3/group/member",
        json!({"startAt":0,"isLast":true,"values":[jira_user("other")]}),
    )
    .await;
    j.refresh_metadata().await.unwrap();
    let new = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert_eq!(new.issue_type.as_deref(), Some("Story"));
    assert_eq!(new.assignees.len(), 1);
    assert_eq!(new.assignees[0].id, "other");
    assert_eq!(
        new.teams
            .iter()
            .find(|t| Some(&t.id) == new.defaults.team.as_ref())
            .unwrap()
            .name,
        "Independent"
    );
    let mut request = create();
    request.purpose = Purpose::Implementation;
    request.assignee_id = Some("me".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );
}

#[tokio::test]
async fn status_inventory_deduplicates_ids_sorts_and_rejects_unknown_or_ambiguous_names_before_create()
 {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    override_get(&server, "/rest/api/3/project/DEV/statuses", json!([
        {"id":"1","name":"Task","statuses":[{"id":"3","name":"Same"},{"id":"1","name":"alpha"},{"id":"2","name":"Same"},{"id":"1","name":"alpha"}]},
        {"id":"2","name":"Story","statuses":[]}
    ])).await;
    let app = editor_fixture(&server).await;
    let j = app.jira.as_ref().unwrap();
    let options = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert_eq!(
        options
            .statuses
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    let count = server.received_requests().await.unwrap().len();
    for status in ["Same", "missing", "1"] {
        let mut request = create();
        request.status = Some(status.into());
        assert_eq!(
            j.create(&request, &pr(), &[], false)
                .await
                .unwrap_err()
                .code,
            "invalid_choice"
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), count);
}

#[tokio::test]
async fn explicit_status_uses_at_most_one_direct_transition_by_destination_id_and_never_recreates()
{
    for scenario in [
        "already",
        "direct",
        "missing",
        "ambiguous",
        "read_failed",
        "transition_read_failed",
        "post_failed",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let app = editor_fixture(&server).await;
        let j = app.jira.as_ref().unwrap();
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-90"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/DEV-90/remotelink"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        if scenario == "read_failed" {
            fail_get(&server, "/rest/api/3/issue/DEV-90").await;
        } else {
            mock_get(&server, "/rest/api/3/issue/DEV-90", json!({"fields":{"status":{"id":if scenario == "already" {"20"} else {"10"},"name":"Not used"}}})).await;
        }
        let mut transitions =
            vec![json!({"id":"wrong","name":"Review","to":{"id":"30","name":"Review"}})];
        if matches!(scenario, "direct" | "ambiguous" | "post_failed") {
            transitions.push(json!({"id":"direct","name":"Arbitrary action name","to":{"id":"20","name":"Different display label"}}));
        }
        if scenario == "ambiguous" {
            transitions.push(json!({"id":"duplicate","to":{"id":"20","name":"Review"}}));
        }
        if scenario == "transition_read_failed" {
            fail_get(&server, "/rest/api/3/issue/DEV-90/transitions").await;
        } else {
            Mock::given(method("GET"))
                .and(path("/rest/api/3/issue/DEV-90/transitions"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"transitions":transitions})),
                )
                .expect(u64::from(!matches!(scenario, "already" | "read_failed")))
                .mount(&server)
                .await;
        }
        let posts = u64::from(matches!(scenario, "direct" | "post_failed"));
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/DEV-90/transitions"))
            .and(body_json(json!({"transition":{"id":"direct"}})))
            .respond_with(ResponseTemplate::new(if scenario == "post_failed" {
                403
            } else {
                204
            }))
            .expect(posts)
            .mount(&server)
            .await;
        let mut request = create();
        request.status = Some("Review".into());
        let result = j.create(&request, &pr(), &[], false).await.unwrap();
        assert_eq!(result.outcome, "created");
        assert_eq!(result.issue.unwrap().key, "DEV-90");
        assert_eq!(
            result.warnings.len(),
            usize::from(!matches!(scenario, "already" | "direct")),
            "{scenario}"
        );
        let log = server.received_requests().await.unwrap();
        let create_body: Value = log
            .iter()
            .find(|r| r.method == "POST" && r.url.path() == "/rest/api/3/issue")
            .unwrap()
            .body_json()
            .unwrap();
        assert!(create_body["fields"].get("status").is_none());
        assert_eq!(
            log.iter()
                .filter(|r| r.method == "POST"
                    && r.url.path() == "/rest/api/3/issue/DEV-90/transitions")
                .count(),
            posts as usize
        );
    }
}

#[tokio::test]
async fn context_uses_yaml_project_order_and_only_sorted_matching_active_sprints_without_changing_choices()
 {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    mock_get(
        &server,
        "/rest/api/3/project/OPS",
        json!({"key":"OPS","name":"Operations"}),
    )
    .await;
    mock_get(
        &server,
        "/rest/api/3/project/OPS/statuses",
        json!([{ "id":"1","name":"Task","statuses":[]}]),
    )
    .await;
    mock_get(
        &server,
        "/rest/api/3/issue/createmeta/OPS/issuetypes",
        json!({"startAt":0,"total":1,"values":[{"id":"1","name":"Task"}]}),
    )
    .await;
    let fields = create_fields();
    mock_get(
        &server,
        "/rest/api/3/issue/createmeta/OPS/issuetypes/1",
        json!({"startAt":0,"total":fields.len(),"fields":fields}),
    )
    .await;
    override_get(
        &server,
        "/rest/agile/1.0/board/12/sprint",
        json!({"startAt":0,"isLast":true,"maxResults":100,"values":[
            {"id":30,"name":"2026-Platform-S1","state":"active"},
            {"id":2,"name":"2025-Platform-S2","state":"active"},
            {"id":10,"name":"2025-Platform-S2","state":"active"},
            {"id":40,"name":"CORE","state":"active","originBoardId":99},
            {"id":50,"name":"2026-Platform-S3","state":"future"}
        ]}),
    )
    .await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.jira.as_mut().unwrap().projects = vec!["OPS".into(), "DEV".into()];
    let app = App::build(c).await.unwrap();
    let (_, context) = call(&app, "GET", "/api/context", json!({})).await;
    assert!(context["current_project"].is_null());
    app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
    let (status, context) = call(&app, "GET", "/api/context", json!({})).await;
    assert_eq!(status, 200);
    validate_contract(&context, "Context");
    assert_eq!(context["current_project"]["key"], "OPS");
    assert_eq!(context["current_project"]["name"], "Operations");
    assert_eq!(
        context["jira_projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["OPS", "DEV"]
    );
    assert_eq!(
        context["current_sprints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["10", "2", "30"]
    );
    for key in ["OPS", "DEV"] {
        let options = app
            .jira
            .as_ref()
            .unwrap()
            .options(key, Purpose::Implementation, None, "")
            .await
            .unwrap();
        assert_eq!(options.sprints.len(), 5);
        assert!(options.defaults.sprint_id.is_none());
    }
    assert_eq!(app.cache.read().await.unwrap().board.sprints.len(), 5);
    let log = server.received_requests().await.unwrap();
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/agile/1.0/board/12/sprint")
            .count(),
        1
    );
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/api/3/users/search")
            .count(),
        2
    );
    assert_eq!(
        log.iter()
            .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search")
            .count(),
        4
    );
    // Later-project discovery cannot stand in for an unavailable first project.
    server.reset().await;
    mock_jira(&server).await;
    let mut c = config();
    c.jira = Some(jira(&server.uri()));
    c.jira.as_mut().unwrap().projects = vec!["OPS".into(), "DEV".into()];
    let app = App::build(c).await.unwrap();
    assert!(app.jira.as_ref().unwrap().refresh_metadata().await.is_err());
    let (_, context) = call(&app, "GET", "/api/context", json!({})).await;
    assert!(context["current_project"].is_null());
    assert_eq!(context["jira_projects"], json!([]));
}

#[tokio::test]
async fn required_editor_values_remain_explicit_blockers_without_filling_assignee_or_sprint() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut fields = create_fields();
    for field in &mut fields {
        if matches!(
            field["fieldId"].as_str(),
            Some("assignee" | "customfield_712" | "customfield_913")
        ) {
            field["required"] = json!(true);
        }
    }
    fields.push(json!({"fieldId":"customfield_711","name":"Sprint","required":true,"schema":{"type":"array","items":"json","custom":"com.pyxis.greenhopper.jira:gh-sprint"},"operations":["set"]}));
    override_get(
        &server,
        "/rest/api/3/issue/createmeta/DEV/issuetypes/1",
        json!({"startAt":0,"total":fields.len(),"fields":fields}),
    )
    .await;
    override_get(&server, "/rest/agile/1.0/board/12/sprint", json!({"startAt":0,"isLast":true,"maxResults":100,"values":[{"id":1,"name":"2026-Platform-S1","state":"active"}]})).await;
    let app = editor_fixture(&server).await;
    let j = app.jira.as_ref().unwrap();
    let options = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert!(options.capabilities.can_create);
    assert!(options.defaults.assignee_id.is_none());
    assert!(options.defaults.sprint_id.is_none());
    for code in [
        "assignee_required",
        "sprint_required",
        "story_points_required",
    ] {
        assert!(options.diagnostics.iter().any(|d| d.code == code));
    }
    let count = server.received_requests().await.unwrap().len();
    let mut request = create();
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "create_blocked"
    );
    request.assignee_id = Some("other".into());
    request.sprint_id = Some("1".into());
    request.story_points = Some(3.0);
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "create_blocked"
    );
    request.team = options.defaults.team;
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap().outcome,
        "simulated"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), count);
}
