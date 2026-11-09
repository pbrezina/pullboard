mod common;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use common::*;
use pullboard::{
    api::{App, router},
    cache::{Cache, SourceData, SourceUpdate},
    config::{ForgeConfig, Upstream},
    forge,
    model::*,
    sync::Progress,
    upstream,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

fn api_root(kind: &str) -> &'static str {
    match kind {
        "github" => "",
        "gitlab" => "/api/v4",
        _ => "/api/v1",
    }
}
fn issue_url(kind: &str, base: &str, number: u64) -> String {
    format!(
        "{base}/acme/widget/{}issues/{number}",
        if kind == "gitlab" { "-/" } else { "" }
    )
}
fn user(kind: &str, id: u64, name: &str) -> Value {
    match kind {
        "github" => github_user(id, name),
        "gitlab" => gitlab_user(id, name),
        _ => forgejo_user(id, name),
    }
}
fn payload(kind: &str, base: &str, number: u64) -> Value {
    let mut v = json!({"number":number,"iid":number,"title":"Exact upstream title","html_url":issue_url(kind,base,number),"web_url":issue_url(kind,base,number),"body":"First\r\nSecond","description":"First\r\nSecond","state":if kind=="gitlab" {"opened"}else{"open"},"user":user(kind,2,"author"),"author":user(kind,2,"author"),"assignees":[user(kind,1,"me"),user(kind,3,"other")],"labels":[{"name":"bug"},{"name":"private"}],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"});
    if kind == "gitlab" {
        v["labels"] = json!(["bug", "private"]);
    }
    v
}
async fn mock(server: &MockServer, p: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(response)
        .mount(server)
        .await;
}
async fn fixture_pr_reads(server: &MockServer, kind: &str) {
    let root = api_root(kind);
    mock(
        server,
        &format!("{root}/user"),
        ResponseTemplate::new(200).set_body_json(user(kind, 1, "me")),
    )
    .await;
    match kind {
        "github" => {
            mock(
                server,
                "/search/issues",
                ResponseTemplate::new(200)
                    .set_body_json(json!({"total_count":0,"incomplete_results":false,"items":[]})),
            )
            .await
        }
        "gitlab" => {
            mock(
                server,
                "/api/v4/merge_requests",
                ResponseTemplate::new(200).set_body_json(json!([])),
            )
            .await;
            mock(
                server,
                "/api/v4/todos",
                ResponseTemplate::new(200).set_body_json(json!([])),
            )
            .await;
        }
        _ => {
            Mock::given(method("GET"))
                .and(path("/api/v1/repos/issues/search"))
                .and(query_param("type", "pulls"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .mount(server)
                .await;
        }
    }
}
async fn assigned_page(server: &MockServer, kind: &str, page: u32, response: ResponseTemplate) {
    let p = format!(
        "{}/{}",
        api_root(kind),
        if matches!(kind, "github" | "gitlab") {
            "issues"
        } else {
            "repos/issues/search"
        }
    );
    let (key, value) = match kind {
        "github" => ("filter", "assigned"),
        "gitlab" => ("scope", "assigned_to_me"),
        _ => ("type", "issues"),
    };
    Mock::given(method("GET"))
        .and(path(p))
        .and(query_param(key, value))
        .and(query_param("page", page.to_string()))
        .respond_with(response)
        .mount(server)
        .await;
}
async fn detail(server: &MockServer, kind: &str, number: u64, v: Value) {
    let p = if kind == "gitlab" {
        format!("/api/v4/projects/acme%2Fwidget/issues/{number}")
    } else {
        format!("{}/repos/acme/widget/issues/{number}", api_root(kind))
    };
    mock(server, &p, ResponseTemplate::new(200).set_body_json(v)).await;
}
fn trusted_issue(config: &ForgeConfig) -> UpstreamIssue {
    let kind = match config.kind {
        Kind::Github => "github",
        Kind::Gitlab => "gitlab",
        Kind::Codeberg => "codeberg",
        _ => "forgejo",
    };
    UpstreamIssue {
        id: format!("{}:acme/widget:issue:7", config.id),
        source_id: config.id.clone(),
        forge: config.kind,
        number: 7,
        owner: "acme".into(),
        repository: "widget".into(),
        title: "Last good issue".into(),
        url: issue_url(kind, &config.base_url, 7),
        description: "Old content".into(),
        state: "open".into(),
        reporter: None,
        assignees: vec![person("me")],
        labels: vec![],
        is_assigned_to_me: true,
        created_at: pr().created_at,
        updated_at: pr().updated_at,
    }
}
async fn call(app: Arc<App>, endpoint: &str, body: Value) -> (StatusCode, Value) {
    let response = router(app)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(endpoint)
                .header("content-type", "application/json")
                .header("x-pullboard-request", "1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let v =
        serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await.unwrap()).unwrap();
    (status, v)
}

#[tokio::test]
async fn all_four_forges_discover_complete_assigned_pages_hydrate_and_exclude_pr_closed_unassigned()
{
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        fixture_pr_reads(&server, kind).await;
        let first = payload(kind, &server.uri(), 7);
        let mut pr_candidate = payload(kind, &server.uri(), 99);
        pr_candidate["pull_request"] = json!({"url":"not fetched"});
        assigned_page(
            &server,
            kind,
            1,
            ResponseTemplate::new(200)
                .set_body_json(json!([first, pr_candidate]))
                .insert_header("x-next-page", "2"),
        )
        .await;
        assigned_page(
            &server,
            kind,
            2,
            ResponseTemplate::new(200)
                .set_body_json(json!([
                    payload(kind, &server.uri(), 7),
                    payload(kind, &server.uri(), 8),
                    payload(kind, &server.uri(), 9),
                    payload(kind, &server.uri(), 10)
                ]))
                .insert_header("x-next-page", ""),
        )
        .await;
        detail(&server, kind, 7, payload(kind, &server.uri(), 7)).await;
        let mut deleted = payload(kind, &server.uri(), 8);
        deleted["user"] = Value::Null;
        deleted["author"] = Value::Null;
        detail(&server, kind, 8, deleted).await;
        let mut closed = payload(kind, &server.uri(), 9);
        closed["state"] = json!("closed");
        detail(&server, kind, 9, closed).await;
        let mut unassigned = payload(kind, &server.uri(), 10);
        unassigned["assignees"] = json!([]);
        detail(&server, kind, 10, unassigned).await;
        let adapter = forge::build(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let progress = Progress::recording("test-forge");
        let data = adapter
            .discover_with_progress(progress.clone())
            .await
            .unwrap();
        assert!(data.pull_requests.is_empty());
        assert_eq!(data.upstream_issues.len(), 2, "{kind}");
        assert_eq!(progress.snapshot().unwrap().completed_items, 4);
        let issue = &data.upstream_issues[0];
        assert_eq!(issue.description, "First\nSecond");
        assert_eq!(issue.assignees.len(), 2);
        assert!(issue.is_assigned_to_me);
        assert_eq!(issue.labels, ["bug", "private"]);
        assert!(data.upstream_issues[1].reporter.is_none());
        validate_contract(&serde_json::to_value(issue).unwrap(), "UpstreamIssue");
        for r in server.received_requests().await.unwrap() {
            assert_eq!(r.method, "GET");
            if r.url.path() == format!("{}/issues", api_root(kind))
                || (r.url.path().ends_with("/repos/issues/search")
                    && r.url
                        .query_pairs()
                        .any(|(k, v)| k == "type" && v == "issues"))
            {
                let query = r.url.query().unwrap();
                assert!(query.contains("assigned"), "{kind}: {query}");
                assert!(!query.contains("scope=all"));
            }
        }
    }
}

#[tokio::test]
async fn all_four_forges_page_failure_retains_both_collections_and_empty_success_replaces_issues() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        fixture_pr_reads(&server, kind).await;
        let f = forge(kind, &server.uri());
        let mut c = config();
        c.forges = vec![f.clone()];
        let cache = Cache::open(std::path::Path::new(":memory:"), &c)
            .await
            .unwrap();
        verify_sources(&cache, &c).await;
        let old = trusted_issue(&f);
        cache
            .publish(vec![SourceUpdate {
                source_id: f.id.clone(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    upstream_issues: vec![old.clone()],
                    ..Default::default()
                }),
            }])
            .await
            .unwrap();
        assigned_page(
            &server,
            kind,
            1,
            ResponseTemplate::new(200)
                .set_body_json(json!([payload(kind, &server.uri(), 8)]))
                .insert_header("x-next-page", "2"),
        )
        .await;
        assigned_page(&server, kind, 2, ResponseTemplate::new(403)).await;
        let adapter = forge::build(&f, &Upstream::default()).await.unwrap();
        let result = adapter.discover().await;
        assert!(result.is_err());
        let snapshot = cache
            .publish(vec![SourceUpdate {
                source_id: f.id.clone(),
                result,
            }])
            .await
            .unwrap();
        assert_eq!(snapshot.board.upstream_issues[0].id, old.id);
        assert_eq!(snapshot.board.pull_requests.len(), 1);
        assert_eq!(snapshot.board.sources[0].status, "error");
        let snapshot = cache
            .publish(vec![SourceUpdate {
                source_id: f.id.clone(),
                result: Ok(SourceData::default()),
            }])
            .await
            .unwrap();
        assert!(snapshot.board.upstream_issues.is_empty());
        assert!(snapshot.board.pull_requests.is_empty());
        assert_eq!(snapshot.board.sources[0].status, "ok");
    }
}

#[tokio::test]
async fn unannotated_clamped_pages_continue_until_empty_and_page_bound_fails_closed() {
    let server = MockServer::start().await;
    let f = forge("github", &server.uri());
    for n in 1..=2 {
        assigned_page(
            &server,
            "github",
            n,
            ResponseTemplate::new(200).set_body_json(json!([payload(
                "github",
                &server.uri(),
                u64::from(n)
            )])),
        )
        .await;
        detail(
            &server,
            "github",
            u64::from(n),
            payload("github", &server.uri(), u64::from(n)),
        )
        .await;
    }
    assigned_page(
        &server,
        "github",
        3,
        ResponseTemplate::new(200).set_body_json(json!([])),
    )
    .await;
    let http = pullboard::transport::ReadTransport::new(&f, &Upstream::default())
        .await
        .unwrap();
    let me = identity("test-forge", Kind::Github, "1");
    assert_eq!(
        upstream::discover(&f, &http, &me, &Progress::default())
            .await
            .unwrap()
            .len(),
        2
    );
    let limits = Upstream {
        max_pages: 1,
        ..Default::default()
    };
    let http = pullboard::transport::ReadTransport::new(&f, &limits)
        .await
        .unwrap();
    assert_eq!(
        upstream::discover(&f, &http, &me, &Progress::default())
            .await
            .unwrap_err()
            .code,
        "pagination_limit"
    );
}

#[test]
fn url_security_fixed_web_mapping_nested_namespaces_and_exact_base_boundaries() {
    let gh = forge("github", "https://api.github.com/");
    assert!(upstream::parse_url(&gh, "https://github.com/acme/widget/issues/1").is_ok());
    assert!(upstream::parse_url(&gh, "https://api.github.com/acme/widget/issues/1").is_err());
    let enterprise = forge("github", "https://git.example/hosting/api/v3/");
    assert!(
        upstream::parse_url(
            &enterprise,
            "https://git.example/hosting/acme/widget/issues/1"
        )
        .is_ok()
    );
    let gl = forge("gitlab", "https://git.example:8443/base");
    let route = upstream::parse_url(
        &gl,
        "https://git.example:8443/base/group/sub/widget/-/issues/8",
    )
    .unwrap();
    assert_eq!(route.owner, "group/sub");
    for input in [
        "https://evil.example/acme/widget/issues/1",
        "http://github.com/acme/widget/issues/1",
        "https://github.com:8443/acme/widget/issues/1",
        "https://me:secret@github.com/acme/widget/issues/1",
        "https://@github.com/acme/widget/issues/1",
        "https://github.com/acme/widget/issues/1?x=1",
        "https://github.com/acme/widget/issues/1#x",
        "https://github.com/acme/../acme/widget/issues/1",
        "https://github.com/acme/%2e%2e/acme/widget/issues/1",
        "https://github.com/acme%2fother/widget/issues/1",
        "https://github.com/acme%5cother/widget/issues/1",
        "https://github.com/acme%252fother/widget/issues/1",
        "https://github.com/acme\\widget/issues/1",
        "https://github.com/acme/widget/issues/0",
        "https://github.com/acme/widget/issues/-1",
        "https://github.com/acme/widget/issues/01",
        "https://github.com/acme/widget/pull/1",
        "https://github.com/acme/widget/pulls/1",
        "https://github.com/acme/widget/issues/1/",
        "https://github.com/acme/widget/issues/1/extra",
    ] {
        assert!(upstream::parse_url(&gh, input).is_err(), "{input}");
    }
    for input in [
        "https://git.example:8443/base-other/group/widget/-/issues/8",
        "https://git.example:8443/group/widget/-/issues/8",
        "https://git.example/base/group/widget/-/issues/8",
        "https://git.example:8443/base/group/widget/-/merge_requests/8",
    ] {
        assert!(upstream::parse_url(&gl, input).is_err(), "{input}");
    }
}

#[tokio::test]
async fn all_four_manual_closed_unassigned_resolutions_are_separate_from_board_and_do_not_need_jira()
 {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        let mut c = config();
        c.forges = vec![forge(kind, &server.uri())];
        let app = App::build(c).await.unwrap();
        let url = issue_url(kind, &server.uri(), 7);
        let (status, v) = call(
            app.clone(),
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":url}),
        )
        .await;
        assert_eq!(status, 503);
        assert_eq!(v["error"]["code"], "upstream_not_ready");
        assert!(server.received_requests().await.unwrap().is_empty());
        verify_sources(&app.cache, &app.config).await;
        let before = serde_json::to_value(app.cache.read().await.unwrap().board).unwrap();
        let mut v = payload(kind, &server.uri(), 7);
        v["state"] = json!("closed");
        v["assignees"] = json!([]);
        detail(&server, kind, 7, v).await;
        let (status, v) = call(
            app.clone(),
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":url}),
        )
        .await;
        assert_eq!(status, 200, "{kind}: {v}");
        validate_contract(&v, "ResolvedUpstreamIssue");
        assert_eq!(v["issue"]["state"], "closed");
        assert_eq!(v["issue"]["is_assigned_to_me"], false);
        assert!(v["warning"].as_str().unwrap().contains("closed"));
        assert_eq!(
            serde_json::to_value(app.cache.read().await.unwrap().board).unwrap(),
            before
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn resolution_rejects_unsafe_ambiguous_policy_and_pr_payloads_without_credential_guessing() {
    let server = MockServer::start().await;
    let mut c = config();
    let f = forge("forgejo", &server.uri());
    let mut second = f.clone();
    second.id = "other-account".into();
    c.forges = vec![f, second];
    let app = App::build(c).await.unwrap();
    let url = issue_url("forgejo", &server.uri(), 7);
    let (status, v) = call(
        app.clone(),
        "/api/upstream-issues/resolve",
        json!({"source_id":null,"url":url}),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(v["error"]["code"], "ambiguous_upstream_source");
    for body in [
        json!({"source_id":"missing","url":url}),
        json!({"source_id":"test-forge","url":"https://evil.example/acme/widget/issues/7"}),
        json!({"source_id":"test-forge","url":format!("{}/acme/widget/pulls/7",server.uri())}),
    ] {
        let (status, v) = call(app.clone(), "/api/upstream-issues/resolve", body).await;
        assert_eq!(status, 422);
        assert_eq!(v["error"]["code"], "invalid_upstream_url");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    verify_sources(&app.cache, &app.config).await;
    let mut v = payload("forgejo", &server.uri(), 7);
    v["pull_request"] = json!({});
    detail(&server, "forgejo", 7, v).await;
    let (status, v) = call(
        app,
        "/api/upstream-issues/resolve",
        json!({"source_id":"test-forge","url":url}),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(v["error"]["code"], "not_upstream_issue");
    let mut c = config();
    let mut f = forge("forgejo", &server.uri());
    f.exclude_repositories = vec!["acme/*".into()];
    c.forges = vec![f];
    let app = App::build(c).await.unwrap();
    let before = server.received_requests().await.unwrap().len();
    let (status, v) = call(
        app,
        "/api/upstream-issues/resolve",
        json!({"source_id":null,"url":url}),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(v["error"]["code"], "repository_not_allowed");
    assert_eq!(server.received_requests().await.unwrap().len(), before);
}

#[tokio::test]
async fn all_four_resolution_transports_reject_redirects_and_validate_returned_browser_routes() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        let target = MockServer::start().await;
        let mut c = config();
        c.forges = vec![forge(kind, &server.uri())];
        let app = App::build(c).await.unwrap();
        verify_sources(&app.cache, &app.config).await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/credential-target", target.uri())),
            )
            .mount(&server)
            .await;
        let (status, _) = call(
            app.clone(),
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":issue_url(kind,&server.uri(),7)}),
        )
        .await;
        assert_eq!(status, 502, "{kind}");
        assert!(target.received_requests().await.unwrap().is_empty());
        server.reset().await;
        let mut v = payload(kind, &server.uri(), 7);
        v["html_url"] = json!(issue_url(kind, &target.uri(), 7));
        v["web_url"] = v["html_url"].clone();
        detail(&server, kind, 7, v).await;
        let (status, _) = call(
            app,
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":issue_url(kind,&server.uri(),7)}),
        )
        .await;
        assert_eq!(status, 502);
    }
}

async fn editor_fixture() -> (Arc<App>, MockServer, MockServer, String) {
    let source = MockServer::start().await;
    let jira_server = MockServer::start().await;
    mock_jira(&jira_server).await;
    let mut c = config();
    c.forges = vec![forge("forgejo", &source.uri())];
    c.forges[0].default_project = Some("DEV".into());
    c.jira = Some(jira(&jira_server.uri()));
    // Any accidental AI/provider request would go to a mock, never a real API.
    c.ai.enabled = true;
    c.ai.base_url = Some(jira_server.uri());
    c.ai.token = Some(serde_yaml::from_str("value: mock-ai-secret").unwrap());
    let app = App::build(c).await.unwrap();
    verify_sources(&app.cache, &app.config).await;
    app.jira.as_ref().unwrap().identity().await.unwrap();
    app.jira.as_ref().unwrap().refresh_metadata().await.unwrap();
    let mut v = payload("forgejo", &source.uri(), 7);
    v["state"] = json!("closed");
    v["assignees"] = json!([]);
    v["title"] = json!("Exact title — no paraphrase");
    detail(&source, "forgejo", 7, v).await;
    let (status, v) = call(
        app.clone(),
        "/api/upstream-issues/resolve",
        json!({"source_id":null,"url":issue_url("forgejo",&source.uri(),7)}),
    )
    .await;
    assert_eq!(status, 200);
    (
        app,
        source,
        jira_server,
        v["issue"]["id"].as_str().unwrap().into(),
    )
}
fn create_body(app: &App, id: &str) -> Value {
    json!({"upstream_issue_id":id,"expected_mode_revision":app.mode.read().mode_revision,"purpose":"implementation","project_key":"DEV","issue_type":"Task","title":"Deliberately edited Jira title","description":"Deliberately edited content","story_points":null,"assignee_id":"me","sprint_id":null,"team":null,"status":null})
}

#[tokio::test]
async fn deterministic_manual_draft_uses_implementation_defaults_and_no_ai_across_refreshes() {
    let (app, source, jira_server, id) = editor_fixture().await;
    // A new board refresh must not replace the scoped manual closed resolution.
    let mut board_issue = trusted_issue(&app.config.forges[0]);
    board_issue.title = "Older open board title".into();
    app.cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                upstream_issues: vec![board_issue],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    let (status, v) = call(
        app.clone(),
        "/api/upstream-issues/ticket-drafts",
        json!({"upstream_issue_id":id,"project_key":null}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    validate_contract(&v, "UpstreamIssueDraft");
    assert_eq!(v["title"], "Exact title — no paraphrase");
    assert_eq!(
        v["description"],
        format!(
            "First\nSecond\n\nUpstream issue: {}\nLabels: bug, private",
            issue_url("forgejo", &source.uri(), 7)
        )
    );
    assert!(v["assignee_id"].is_null());
    assert_eq!(v["purpose"], "implementation");
    assert!(v["suggested_status"].is_null());
    assert!(v["story_points"].is_null());
    assert!(v["warning"].as_str().unwrap().contains("closed"));
    assert!(
        jira_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
    let (status, _) = call(
        app,
        "/api/upstream-issues/ticket-drafts",
        json!({"upstream_issue_id":id,"project_key":"NOT_CONFIGURED"}),
    )
    .await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn manual_simulated_create_has_zero_writes_and_no_cache_or_sync_change() {
    let (app, _source, jira_server, id) = editor_fixture().await;
    let before = serde_json::to_value(app.cache.read().await.unwrap().board).unwrap();
    let sync = serde_json::to_value(app.sync.status().await.unwrap()).unwrap();
    let (status, v) = call(
        app.clone(),
        "/api/upstream-issues/jira-issues",
        create_body(&app, &id),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    validate_contract(&v, "CreateIssueResult");
    assert_eq!(v["outcome"], "simulated");
    assert!(v["issue"].is_null());
    assert!(v["message"].as_str().unwrap().contains("simulated"));
    assert_eq!(
        serde_json::to_value(app.cache.read().await.unwrap().board).unwrap(),
        before
    );
    assert_eq!(
        serde_json::to_value(app.sync.status().await.unwrap()).unwrap(),
        sync
    );
    assert!(
        jira_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn live_create_uses_edited_values_and_trusted_issue_weblink_then_schedules_refresh() {
    let (app, source, jira_server, id) = editor_fixture().await;
    let revision = app.mode.read().mode_revision;
    app.mode.change(&revision, false).unwrap();
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .and(body_partial_json(
            json!({"fields":{"summary":"Deliberately edited Jira title"}}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-91"})))
        .expect(1)
        .mount(&jira_server)
        .await;
    Mock::given(method("POST")).and(path("/rest/api/3/issue/DEV-91/remotelink"))
        .and(body_partial_json(json!({"object":{"url":issue_url("forgejo",&source.uri(),7),"title":"Exact title — no paraphrase"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).expect(1).mount(&jira_server).await;
    let (status, v) = call(
        app.clone(),
        "/api/upstream-issues/jira-issues",
        create_body(&app, &id),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["outcome"], "created");
    assert_eq!(v["issue"]["key"], "DEV-91");
    assert_eq!(v["warnings"], json!([]));
    assert_eq!(app.sync.status().await.unwrap().state, "syncing");
    assert!(
        source
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn accepted_creation_unknown_key_or_failed_link_is_created_success_not_duplicate_inviting_error()
 {
    for body in [
        json!({"key":"DEV-92"}),
        json!({"id":"accepted-without-key"}),
    ] {
        let (app, _source, jira_server, id) = editor_fixture().await;
        app.mode
            .change(&app.mode.read().mode_revision, false)
            .unwrap();
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(body.clone()))
            .expect(1)
            .mount(&jira_server)
            .await;
        let (status, v) = call(
            app.clone(),
            "/api/upstream-issues/jira-issues",
            create_body(&app, &id),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["outcome"], "created");
        assert!(!v["warnings"].as_array().unwrap().is_empty());
        assert_eq!(v["issue"].is_null(), body.get("key").is_none());
    }
}

#[tokio::test]
async fn mode_conflict_precedes_selection_validation_and_requests_but_structural_errors_still_reject()
 {
    let (app, _source, jira_server, id) = editor_fixture().await;
    let mut body = create_body(&app, &id);
    let before = jira_server.received_requests().await.unwrap().len();
    app.mode
        .change(&app.mode.read().mode_revision, false)
        .unwrap();
    body["upstream_issue_id"] = json!("missing");
    body["title"] = json!("");
    body["project_key"] = json!("NOPE");
    let (status, v) = call(app.clone(), "/api/upstream-issues/jira-issues", body).await;
    assert_eq!(status, 409);
    assert_eq!(v["error"]["code"], "mode_conflict");
    assert_eq!(jira_server.received_requests().await.unwrap().len(), before);
    for field in ["story_points", "assignee_id", "sprint_id", "team", "status"] {
        let mut body = create_body(&app, &id);
        body.as_object_mut().unwrap().remove(field);
        let (status, _) = call(app.clone(), "/api/upstream-issues/jira-issues", body).await;
        assert_eq!(status, 400);
    }
    for (field, value) in [
        ("purpose", json!("review")),
        ("client_url", json!("https://evil.example")),
        ("pull_request_id", json!("not permitted")),
    ] {
        let mut body = create_body(&app, &id);
        body[field] = value;
        let (status, _) = call(app.clone(), "/api/upstream-issues/jira-issues", body).await;
        assert_eq!(status, 400);
    }
}

#[tokio::test]
async fn account_change_quarantines_board_and_invalidates_manual_private_selection() {
    let (app, _source, _jira_server, id) = editor_fixture().await;
    app.cache
        .verify(identity("test-forge", Kind::Forgejo, "different-account"))
        .await
        .unwrap();
    // Returning to the former principal must not resurrect a prior private lookup,
    // even if no lookup occurred while the other principal was current.
    app.cache
        .verify(identity("test-forge", Kind::Forgejo, "1"))
        .await
        .unwrap();
    let (status, v) = call(
        app,
        "/api/upstream-issues/ticket-drafts",
        json!({"upstream_issue_id":id,"project_key":null}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(v["error"]["code"], "upstream_issue_not_found");
}

#[test]
fn manual_store_is_bounded_and_scoped_by_service_policy_and_principal_not_textual_id() {
    let f = forge("forgejo", "https://forge.example");
    let store = upstream::Resolutions::default();
    let mut snapshot = pullboard::cache::Snapshot::default();
    snapshot
        .source_accounts
        .insert(f.id.clone(), "scope-a".into());
    for n in 0..129 {
        let mut i = trusted_issue(&f);
        i.id = format!("issue-{n}");
        store.insert("scope-a".into(), i).unwrap();
    }
    assert_eq!(
        store.select("issue-0", &snapshot).unwrap_err().code,
        "upstream_issue_not_found"
    );
    assert!(store.select("issue-128", &snapshot).is_ok());
    snapshot
        .source_accounts
        .insert(f.id.clone(), "changed-service-policy-or-account".into());
    assert!(store.select("issue-128", &snapshot).is_err());
    snapshot
        .source_accounts
        .insert(f.id.clone(), "scope-a".into());
    assert!(store.select("issue-128", &snapshot).is_err());
}

#[test]
fn deterministic_text_preserves_long_title_and_description_and_handles_empty_description() {
    let f = forge("forgejo", "https://forge.example");
    let mut issue = trusted_issue(&f);
    issue.title = "x".repeat(300);
    issue.description = "z".repeat(100001);
    assert!(upstream::description(&issue).starts_with(&issue.description));
    assert_eq!(issue.title.len(), 300);
    issue.description.clear();
    issue.labels = vec!["a".into(), "b".into()];
    assert_eq!(
        upstream::description(&issue),
        format!("Upstream issue: {}\nLabels: a, b", issue.url)
    );
}

#[tokio::test]
async fn all_four_forges_exclude_hydrated_pr_markers_but_manual_resolution_rejects_them() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        fixture_pr_reads(&server, kind).await;
        assigned_page(
            &server,
            kind,
            1,
            ResponseTemplate::new(200)
                .set_body_json(json!([payload(kind, &server.uri(), 7)]))
                .insert_header("x-total-count", "1"),
        )
        .await;
        let mut value = payload(kind, &server.uri(), 7);
        value[if kind == "gitlab" {
            "merge_request"
        } else {
            "pull_request"
        }] = json!({});
        detail(&server, kind, 7, value).await;
        let f = forge(kind, &server.uri());
        let adapter = forge::build(&f, &Upstream::default()).await.unwrap();
        let progress = Progress::recording("test-forge");
        assert!(
            adapter
                .discover_with_progress(progress.clone())
                .await
                .unwrap()
                .upstream_issues
                .is_empty()
        );
        assert_eq!(progress.snapshot().unwrap().completed_items, 1);
        let route = upstream::parse_url(&f, &issue_url(kind, &server.uri(), 7)).unwrap();
        assert_eq!(
            adapter
                .issue(&route, &identity(&f.id, f.kind, "1"))
                .await
                .unwrap_err()
                .code,
            "not_upstream_issue"
        );
    }
}

#[tokio::test]
async fn resolution_maps_missing_permission_and_backoff_errors_and_preserves_other_entries() {
    for (upstream_status, expected, code) in [
        (404, 404, "upstream_issue_not_found"),
        (403, 502, "upstream_failure"),
        (429, 429, "upstream_backoff"),
    ] {
        let (app, source, _jira, id) = editor_fixture().await;
        source.reset().await;
        let response = if upstream_status == 429 {
            ResponseTemplate::new(upstream_status).insert_header("retry-after", "60")
        } else {
            ResponseTemplate::new(upstream_status)
        };
        Mock::given(method("GET"))
            .respond_with(response)
            .mount(&source)
            .await;
        let before = serde_json::to_value(app.cache.read().await.unwrap().board).unwrap();
        let (status, value) = call(
            app.clone(),
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":issue_url("forgejo",&source.uri(),8)}),
        )
        .await;
        assert_eq!(status.as_u16(), expected);
        assert_eq!(value["error"]["code"], code);
        let (status, _) = call(
            app.clone(),
            "/api/upstream-issues/ticket-drafts",
            json!({"upstream_issue_id":id,"project_key":null}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::to_value(app.cache.read().await.unwrap().board).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn deterministic_editor_does_not_truncate_overlong_content_and_final_create_requires_editing()
{
    let (app, source, jira_server, id) = editor_fixture().await;
    source.reset().await;
    let mut value = payload("forgejo", &source.uri(), 7);
    value["title"] = json!("t".repeat(256));
    value["body"] = json!("d".repeat(100001));
    detail(&source, "forgejo", 7, value).await;
    assert_eq!(
        call(
            app.clone(),
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":issue_url("forgejo",&source.uri(),7)})
        )
        .await
        .0,
        200
    );
    let (status, draft) = call(
        app.clone(),
        "/api/upstream-issues/ticket-drafts",
        json!({"upstream_issue_id":id,"project_key":null}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(draft["title"].as_str().unwrap().len(), 256);
    assert!(draft["description"].as_str().unwrap().len() > 100001);
    let mut body = create_body(&app, &id);
    body["title"] = draft["title"].clone();
    body["description"] = draft["description"].clone();
    assert_eq!(
        call(app, "/api/upstream-issues/jira-issues", body).await.0,
        422
    );
    assert!(
        jira_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn cache_restart_quarantines_both_forge_collections_until_exact_account_service_policy_verification()
 {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.sqlite");
    let mut c = config();
    c.forges = vec![forge("forgejo", "https://forge.example")];
    let cache = Cache::open(&db, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                pull_requests: vec![pr()],
                upstream_issues: vec![trusted_issue(&c.forges[0])],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    drop(cache);
    let cache = Cache::open(&db, &c).await.unwrap();
    assert!(cache.read().await.unwrap().board.upstream_issues.is_empty());
    verify_sources(&cache, &c).await;
    let board = cache.read().await.unwrap().board;
    assert_eq!(board.upstream_issues.len(), 1);
    assert_eq!(board.pull_requests.len(), 1);
    assert_eq!(board.sources[0].status, "error");
    drop(cache);
    c.forges[0].exclude_repositories = vec!["acme/*".into()];
    let cache = Cache::open(&db, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    let board = cache.read().await.unwrap().board;
    assert!(board.upstream_issues.is_empty());
    assert!(board.pull_requests.is_empty());
}

#[tokio::test]
async fn manual_fetch_uses_only_configured_api_base_including_nested_gitlab_namespace_and_enterprise_mapping()
 {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        let base = format!(
            "{}/hosting{}",
            server.uri(),
            if kind == "github" { "/api/v3" } else { "" }
        );
        let mut c = config();
        c.forges = vec![forge(kind, &base)];
        let app = App::build(c).await.unwrap();
        verify_sources(&app.cache, &app.config).await;
        let web = format!("{}/hosting", server.uri());
        let mut value = payload(kind, &web, 7);
        let url = if kind == "gitlab" {
            format!("{web}/group/sub/widget/-/issues/7")
        } else {
            issue_url(kind, &web, 7)
        };
        value["web_url"] = json!(url);
        value["html_url"] = json!(url);
        let api_path = match kind {
            "github" => "/hosting/api/v3/repos/acme/widget/issues/7",
            "gitlab" => "/hosting/api/v4/projects/group%2Fsub%2Fwidget/issues/7",
            _ => "/hosting/api/v1/repos/acme/widget/issues/7",
        };
        mock(
            &server,
            api_path,
            ResponseTemplate::new(200).set_body_json(value),
        )
        .await;
        let (status, value) = call(
            app,
            "/api/upstream-issues/resolve",
            json!({"source_id":null,"url":url}),
        )
        .await;
        assert_eq!(status, 200, "{kind}: {value}");
        if kind == "gitlab" {
            assert_eq!(value["issue"]["owner"], "group/sub");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn contradictory_pagination_metadata_fails_instead_of_silently_publishing_a_short_inventory()
{
    let server = MockServer::start().await;
    let f = forge("forgejo", &server.uri());
    assigned_page(
        &server,
        "forgejo",
        1,
        ResponseTemplate::new(200)
            .set_body_json(json!([payload("forgejo", &server.uri(), 7)]))
            .insert_header("x-next-page", "")
            .insert_header("x-total-count", "2"),
    )
    .await;
    let http = pullboard::transport::ReadTransport::new(&f, &Upstream::default())
        .await
        .unwrap();
    assert!(
        upstream::discover(
            &f,
            &http,
            &identity(&f.id, f.kind, "1"),
            &Progress::default()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn repeated_assigned_pages_cannot_satisfy_total_count_by_counting_the_same_issues_twice() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        for (next_metadata, first_total_only) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let server = MockServer::start().await;
            let f = forge(kind, &server.uri());
            for page in [1, 2] {
                let mut response = ResponseTemplate::new(200).set_body_json(json!([
                    payload(kind, &server.uri(), 7),
                    payload(kind, &server.uri(), 8)
                ]));
                if page == 1 || !first_total_only {
                    response = response.insert_header("x-total-count", "4");
                }
                if next_metadata {
                    response =
                        response.insert_header("x-next-page", if page == 1 { "2" } else { "" });
                }
                assigned_page(&server, kind, page, response).await;
            }
            let mut final_response = ResponseTemplate::new(200).set_body_json(json!([]));
            if !first_total_only {
                final_response = final_response.insert_header("x-total-count", "4");
            }
            assigned_page(&server, kind, 3, final_response).await;
            // Valid detail fixtures keep a missing mock from masking false success.
            for number in [7, 8] {
                detail(&server, kind, number, payload(kind, &server.uri(), number)).await;
            }
            let http = pullboard::transport::ReadTransport::new(&f, &Upstream::default())
                .await
                .unwrap();
            assert!(
                upstream::discover(
                    &f,
                    &http,
                    &identity(&f.id, f.kind, "1"),
                    &Progress::default()
                )
                .await
                .is_err(),
                "{kind}"
            );
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                if next_metadata { 2 } else { 3 }
            );
        }
    }
}

#[tokio::test]
async fn overlapping_assigned_pages_with_an_honest_unique_total_remain_supported() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        let f = forge(kind, &server.uri());
        for (page, numbers, next) in [(1, [7, 8], "2"), (2, [8, 9], "")] {
            assigned_page(
                &server,
                kind,
                page,
                ResponseTemplate::new(200)
                    .set_body_json(json!(numbers.map(|n| payload(kind, &server.uri(), n))))
                    .insert_header("x-total-count", "3")
                    .insert_header("x-next-page", next),
            )
            .await;
        }
        for number in [7, 8, 9] {
            detail(&server, kind, number, payload(kind, &server.uri(), number)).await;
        }
        let http = pullboard::transport::ReadTransport::new(&f, &Upstream::default())
            .await
            .unwrap();
        assert_eq!(
            upstream::discover(
                &f,
                &http,
                &identity(&f.id, f.kind, "1"),
                &Progress::default()
            )
            .await
            .unwrap()
            .len(),
            3
        );
    }
}

#[tokio::test]
async fn sync_issue_read_failures_keep_both_last_good_collections_with_shared_health_and_honest_progress()
 {
    let mut c = config();
    let mut servers = vec![];
    for (index, kind) in ["github", "gitlab", "codeberg", "forgejo"]
        .into_iter()
        .enumerate()
    {
        let server = MockServer::start().await;
        fixture_pr_reads(&server, kind).await;
        assigned_page(&server, kind, 1, ResponseTemplate::new(403)).await;
        let mut f = forge(kind, &server.uri());
        f.id = format!("instance-{index}");
        c.forges.push(f);
        servers.push(server);
    }
    let app = App::build(c).await.unwrap();
    verify_sources(&app.cache, &app.config).await;
    for f in &app.config.forges {
        let mut pull = pr();
        pull.source_id = f.id.clone();
        pull.id = format!("{}:acme/widget:42", f.id);
        pull.forge = f.kind;
        app.cache
            .publish(vec![SourceUpdate {
                source_id: f.id.clone(),
                result: Ok(SourceData {
                    pull_requests: vec![pull],
                    upstream_issues: vec![trusted_issue(f)],
                    ..Default::default()
                }),
            }])
            .await
            .unwrap();
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let status = app.sync.status().await.unwrap();
            if status.state == "idle" && status.finished_at.is_some() {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    worker.await.unwrap();
    let board = app.cache.read().await.unwrap().board;
    assert_eq!(board.pull_requests.len(), 4);
    assert_eq!(board.upstream_issues.len(), 4);
    assert!(board.sources.iter().all(|s| s.status == "error"));
    assert!(
        status
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Failed
                && p.completed_items == 0
                && p.total_items.is_none())
    );
    assert!(
        status
            .sources
            .iter()
            .all(|s| s.last_success_at.is_some() && s.status == "error")
    );
}
