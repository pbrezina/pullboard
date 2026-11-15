//! HTTP body barriers, not speed estimates: concurrent discovery and queue limits.
mod common;

use async_trait::async_trait;
use axum::{Router, body::Body, extract::State, response::Response};
use common::*;
use futures_util::stream;
use pullboard::{
    api::App,
    cache::{Cache, SourceData},
    error::Result,
    forge::Forge,
    jira::Jira,
    model::*,
    sync::{Progress, Syncer},
};
use serde_json::json;
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn concurrent_contract_example_roundtrips_without_alternate_progress_shapes() {
    let example: serde_json::Value = serde_json::from_str(include_str!(
        "../../api/examples/sync-options-concurrent.json"
    ))
    .unwrap();
    validate_contract(&example, "SyncStatus");
    let status: SyncStatus = serde_json::from_value(example.clone()).unwrap();
    assert_eq!(serde_json::to_value(status).unwrap(), example);
}

struct Gates {
    metadata: String,
    client: reqwest::Client,
    searches: Semaphore,
    sprint: Semaphore,
    hydration: Semaphore,
    discovery: Semaphore,
    eligibility: Semaphore,
    groups: Semaphore,
    block_groups: AtomicBool,
    events: mpsc::UnboundedSender<String>,
    requests: Mutex<Vec<String>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    active_checks: AtomicUsize,
    peak_checks: AtomicUsize,
    fail_checks: AtomicBool,
    rate_limit_checks: AtomicBool,
    fail_metadata: AtomicBool,
    fail_tickets: AtomicBool,
    fail_sprints: AtomicBool,
    changed_identity: AtomicBool,
    sprint_id: AtomicUsize,
}
struct Active(Arc<Gates>, bool);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        if self.1 {
            self.0.active_checks.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
async fn handler(State(state): State<Arc<Gates>>, request: axum::extract::Request) -> Response {
    assert_eq!(
        request.method(),
        "GET",
        "background refresh must never write"
    );
    let uri = request.uri().to_string();
    let path = request.uri().path().to_owned();
    let params: Vec<_> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let param = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    };
    let label = if path.ends_with("/assignable/search") {
        format!("check:{}:{}", param("project"), param("accountId"))
    } else if path.ends_with("/group/member") && state.block_groups.load(Ordering::SeqCst) {
        format!("group:{}:{}", param("groupId"), param("startAt"))
    } else if path.ends_with("/search/jql") {
        if param("jql").contains("assignee =") {
            "assigned".into()
        } else {
            "mentions".into()
        }
    } else {
        path.clone()
    };
    state.requests.lock().unwrap().push(label.clone());
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    let is_check = path.ends_with("/assignable/search");
    if is_check {
        let active = state.active_checks.fetch_add(1, Ordering::SeqCst) + 1;
        state.peak_checks.fetch_max(active, Ordering::SeqCst);
    }
    let guard = Active(state.clone(), is_check);
    let (status, bytes) = if path.ends_with("/myself") {
        (
            200,
            serde_json::to_vec(&jira_user(
                if state.changed_identity.load(Ordering::SeqCst) {
                    "new"
                } else {
                    "me"
                },
            ))
            .unwrap(),
        )
    } else if path.ends_with("/sprint") {
        if state.fail_sprints.load(Ordering::SeqCst) {
            (200, b"{}".to_vec())
        } else {
            (200, serde_json::to_vec(&json!({"startAt":0,"maxResults":100,"isLast":true,"values":[{"id":state.sprint_id.load(Ordering::SeqCst),"name":"2026-Platform-S1","state":"active"}]})).unwrap())
        }
    } else if (path.ends_with("/configuration") && state.fail_metadata.load(Ordering::SeqCst))
        || (path.ends_with("/comment") && state.fail_tickets.load(Ordering::SeqCst))
    {
        (200, b"{}".to_vec())
    } else if path.ends_with("/search/jql") {
        (200, serde_json::to_vec(&json!({"isLast":true,"issues":[{"id":"1","key":"DEV-1","fields":{"summary":"Published ticket","project":{"key":"DEV"},"issuetype":{"name":"Task"},"status":{"name":"Open"},"assignee":jira_user("me"),"created":"2026-01-01T00:00:00Z","updated":"2026-01-02T00:00:00Z","customfield_711":[{"id":1}]}}]})).unwrap())
    } else if path.ends_with("/assignable/search") && state.rate_limit_checks.load(Ordering::SeqCst)
    {
        (429, b"{}".to_vec())
    } else if path.ends_with("/assignable/search") && state.fail_checks.load(Ordering::SeqCst) {
        (200, b"{\"private upstream error\":true}".to_vec())
    } else {
        let response = state
            .client
            .get(format!("{}{}", state.metadata, uri))
            .send()
            .await
            .unwrap();
        (
            response.status().as_u16(),
            response.bytes().await.unwrap().to_vec(),
        )
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("retry-after", "300")
        .body(Body::from_stream(stream::once(async move {
            let _guard = guard;
            let gate = if path.ends_with("/search/jql") {
                Some(&state.searches)
            } else if path.ends_with("/sprint") {
                Some(&state.sprint)
            } else if path.ends_with("/comment") {
                Some(&state.hydration)
            } else if path.ends_with("/configuration") {
                Some(&state.discovery)
            } else if path.ends_with("/assignable/search") {
                Some(&state.eligibility)
            } else if path.ends_with("/group/member") && state.block_groups.load(Ordering::SeqCst) {
                Some(&state.groups)
            } else {
                None
            };
            if let Some(gate) = gate {
                let _ = state.events.send(label);
                gate.acquire().await.unwrap().forget();
            }
            Ok::<_, Infallible>(bytes)
        })))
        .unwrap()
}

struct Fixture {
    base: String,
    state: Arc<Gates>,
    events: mpsc::UnboundedReceiver<String>,
    server: tokio::task::JoinHandle<()>,
    _metadata: MockServer,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let metadata = MockServer::start().await;
        mock_jira(&metadata).await;
        mock_get(
            &metadata,
            "/rest/api/3/issue/DEV-1/comment",
            json!({"startAt":0,"maxResults":2,"total":0,"comments":[]}),
        )
        .await;
        mock_get(&metadata, "/rest/api/3/issue/DEV-1/remotelink", json!([])).await;
        mock_get(
            &metadata,
            "/rest/api/3/project/OPS",
            json!({"key":"OPS","name":"Operations"}),
        )
        .await;
        mock_get(
            &metadata,
            "/rest/api/3/project/OPS/statuses",
            json!([{"id":"1","name":"Task","statuses":[]}]),
        )
        .await;
        mock_get(
            &metadata,
            "/rest/api/3/issue/createmeta/OPS/issuetypes",
            json!({"startAt":0,"isLast":true,"values":[{"id":"1","name":"Task"}]}),
        )
        .await;
        let fields = create_fields();
        mock_get(
            &metadata,
            "/rest/api/3/issue/createmeta/OPS/issuetypes/1",
            json!({"startAt":0,"isLast":true,"fields":fields}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/group/bulk"))
            .respond_with(|request: &wiremock::Request| {
                let name = request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "groupName")
                    .unwrap()
                    .1
                    .into_owned();
                ResponseTemplate::new(200).set_body_json(
                    json!({"startAt":0,"isLast":true,"values":[{"name":name,"groupId":name}]}),
                )
            })
            .mount(&metadata)
            .await;
        Mock::given(method("GET")).and(path("/rest/api/3/group/member")).respond_with(|request: &wiremock::Request| {
            let param = |key: &str| request.url.query_pairs().find(|(k, _)| k == key).unwrap().1.into_owned();
            let users = if param("groupId") == "Group B" { vec![jira_user("other"), jira_user("third")] } else { vec![jira_user("me"), jira_user("other")] };
            // Two real pages; overlap is deduplicated before project checks.
            let start: usize = param("startAt").parse().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"startAt":start,"maxResults":1,"total":2,"values":users.into_iter().skip(start).take(1).collect::<Vec<_>>()}))
        }).mount(&metadata).await;
        let (tx, events) = mpsc::unbounded_channel();
        let state = Arc::new(Gates {
            metadata: metadata.uri(),
            client: reqwest::Client::new(),
            searches: Semaphore::new(0),
            sprint: Semaphore::new(0),
            hydration: Semaphore::new(0),
            discovery: Semaphore::new(0),
            eligibility: Semaphore::new(0),
            groups: Semaphore::new(0),
            block_groups: AtomicBool::new(false),
            events: tx,
            requests: Mutex::new(vec![]),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            active_checks: AtomicUsize::new(0),
            peak_checks: AtomicUsize::new(0),
            fail_checks: AtomicBool::new(false),
            rate_limit_checks: AtomicBool::new(false),
            fail_metadata: AtomicBool::new(false),
            fail_tickets: AtomicBool::new(false),
            fail_sprints: AtomicBool::new(false),
            changed_identity: AtomicBool::new(false),
            sprint_id: AtomicUsize::new(1),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(handler).with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            base,
            state,
            events,
            server,
            _metadata: metadata,
        }
    }
    fn config(&self) -> pullboard::config::Config {
        let mut c = config();
        let mut j = jira(&self.base);
        j.projects = vec!["OPS".into(), "DEV".into()];
        j.assignee_groups = vec!["Group B".into(), "Group A".into(), "Group B".into()];
        c.jira = Some(j);
        c.upstream.max_concurrent_requests = 3;
        c.upstream.max_concurrent_items = 2;
        c
    }
    fn open_tickets(&self) {
        self.state.searches.add_permits(100);
        self.state.sprint.add_permits(100);
        self.state.hydration.add_permits(100);
    }
    async fn event(&mut self) -> String {
        tokio::time::timeout(DEADLINE, self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn until_event(&mut self, expected: &str) {
        while self.event().await != expected {}
    }
    fn check_count(&self) -> usize {
        self.state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("check:"))
            .count()
    }
}
async fn status(sync: &Syncer, predicate: impl Fn(&SyncStatus) -> bool) -> SyncStatus {
    tokio::time::timeout(DEADLINE, async {
        loop {
            let status = sync.status().await.unwrap();
            validate_contract(&serde_json::to_value(&status).unwrap(), "SyncStatus");
            if predicate(&status) {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

struct GateForge(Arc<Semaphore>);
#[async_trait]
impl Forge for GateForge {
    fn source_id(&self) -> &str {
        "test-forge"
    }
    async fn identity(&self) -> Result<Identity> {
        Ok(identity("test-forge", Kind::Github, "1"))
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        progress.fetching(Some(1));
        self.0.acquire().await.unwrap().forget();
        progress.item_completed();
        Ok(SourceData {
            pull_requests: vec![pr()],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn eligibility_overlaps_cards_and_metadata_then_expands_without_waiting_for_the_slow_body() {
    let mut fixture = Fixture::new().await;
    let mut c = fixture.config();
    c.upstream.max_concurrent_requests = 4;
    c.forges = vec![forge("github", "https://forge.invalid")];
    fixture.state.block_groups.store(true, Ordering::SeqCst);
    fixture.state.searches.add_permits(100);
    fixture.state.sprint.add_permits(100);
    let jira = Jira::new(c.jira.as_ref().unwrap(), &c.upstream)
        .await
        .unwrap();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let forge_gate = Arc::new(Semaphore::new(0));
    let sync = Syncer::new(
        cache.clone(),
        vec![Arc::new(GateForge(forge_gate.clone()))],
        Some(jira.clone()),
        3600,
    );
    let initial = sync.status().await.unwrap();
    assert!(!initial.jira_options.as_ref().unwrap().ready);
    validate_contract(&serde_json::to_value(initial).unwrap(), "SyncStatus");
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    let expected: std::collections::BTreeSet<_> = [
        "/rest/api/3/issue/DEV-1/comment".to_string(),
        "/rest/agile/1.0/board/12/configuration".to_string(),
        "group:Group A:0".to_string(),
        "group:Group B:0".to_string(),
    ]
    .into_iter()
    .collect();
    let mut observed = std::collections::BTreeSet::new();
    while !expected.is_subset(&observed) {
        observed.insert(fixture.event().await);
    }
    assert_eq!(fixture.check_count(), 0);
    fixture.state.groups.add_permits(2);
    let mut second = std::collections::BTreeSet::new();
    while second.len() < 2 {
        let event = fixture.event().await;
        if event.starts_with("group:") {
            assert!(event.ends_with(":1"));
            second.insert(event);
        }
    }
    assert_eq!(
        fixture.check_count(),
        0,
        "partial group pages never start checks"
    );
    fixture.state.groups.add_permits(1);
    status(&sync, |s| {
        s.jira_options
            .as_ref()
            .unwrap()
            .progress
            .groups
            .iter()
            .filter(|r| r.phase == SourcePhase::Completed)
            .count()
            == 1
    })
    .await;
    assert_eq!(fixture.check_count(), 0, "every group must be complete");
    fixture.state.groups.add_permits(1);
    fixture.until_event("check:OPS:me").await;
    let concurrent = status(&sync, |s| {
        s.jira_options
            .as_ref()
            .unwrap()
            .progress
            .projects
            .iter()
            .any(|r| r.phase == SourcePhase::Fetching)
    })
    .await;
    assert_eq!(concurrent.progress[0].phase, SourcePhase::Fetching);
    assert_eq!(
        concurrent.jira_options.as_ref().unwrap().progress.phase,
        SourcePhase::Discovering
    );
    assert_eq!(fixture.check_count(), 1);
    assert_eq!(fixture.state.peak_checks.load(Ordering::SeqCst), 1);
    fixture.state.hydration.add_permits(1);
    let early = status(&sync, |s| s.progress[0].phase == SourcePhase::Completed).await;
    assert_eq!(early.state, "syncing");
    assert_eq!(
        early.progress[0].completed_items, 1,
        "duplicate searches hydrate once"
    );
    assert_eq!(early.progress[1].phase, SourcePhase::Fetching);
    assert!(!early.jira_options.as_ref().unwrap().ready);
    let board = cache.read().await.unwrap().board;
    assert_eq!(early.board_revision, board.revision);
    assert_eq!(board.issues[0].title, "Published ticket");
    assert_eq!(board.sprints[0].id, "1");
    assert_eq!(
        cache.read().await.unwrap().jira_project.unwrap().name,
        "Operations"
    );
    assert_eq!(
        jira.options("OPS", Purpose::Review, None, "")
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    assert_eq!(
        jira.create(&create(), &pr(), &board.sprints, true)
            .await
            .unwrap_err()
            .code,
        "jira_not_ready"
    );
    assert_eq!(
        fixture.check_count(),
        1,
        "another card source is still blocked"
    );
    forge_gate.add_permits(1);
    status(&sync, |s| {
        s.progress.iter().all(|p| p.phase == SourcePhase::Completed)
    })
    .await;
    fixture.until_event("check:DEV:me").await;
    assert_eq!(
        fixture.check_count(),
        2,
        "last card publication wakes expansion even while the first eligibility body and metadata are blocked"
    );
    assert_eq!(fixture.state.active_checks.load(Ordering::SeqCst), 2);
    fixture.state.discovery.add_permits(1);
    let fetching = status(&sync, |s| {
        s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Fetching
    })
    .await;
    let options = fetching.jira_options.as_ref().unwrap();
    assert_eq!(
        fixture.check_count(),
        2,
        "one queue honors the configured item bound"
    );
    assert_eq!(
        options
            .progress
            .groups
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["Group B", "Group A"]
    );
    assert!(
        options
            .progress
            .groups
            .iter()
            .all(|r| r.phase == SourcePhase::Completed
                && r.completed_items == 2
                && r.total_items == Some(2))
    );
    assert_eq!(
        options
            .progress
            .projects
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["OPS", "DEV"]
    );
    assert!(
        options
            .progress
            .projects
            .iter()
            .all(|r| r.completed_items == 0 && r.total_items == Some(3))
    );
    assert_eq!(
        fetching.progress[0].elapsed_ms,
        early.progress[0].elapsed_ms
    );
    let coalesced = sync.request().await.unwrap();
    assert_eq!(coalesced.started_at, fetching.started_at);
    assert_eq!(
        coalesced.jira_options.as_ref().unwrap().progress.started_at,
        options.progress.started_at
    );
    fixture.state.eligibility.add_permits(1);
    assert!(fixture.event().await.starts_with("check:"));
    let partial = status(&sync, |s| {
        s.jira_options
            .as_ref()
            .unwrap()
            .progress
            .projects
            .iter()
            .map(|r| r.completed_items)
            .sum::<u64>()
            == 1
    })
    .await;
    assert!(!partial.jira_options.unwrap().ready);
    assert!(cache.read().await.unwrap().jira_choices.is_none());
    fixture.state.eligibility.add_permits(20);
    let done = status(&sync, |s| s.state == "idle").await;
    assert!(done.last_success_at.is_some());
    assert!(done.jira_options.as_ref().unwrap().ready);
    assert!(
        done.jira_options
            .as_ref()
            .unwrap()
            .progress
            .projects
            .iter()
            .all(|r| r.completed_items == 3
                && r.total_items == Some(3)
                && r.phase == SourcePhase::Completed)
    );
    assert_eq!(
        fixture.check_count(),
        6,
        "overlapping group accounts are checked once per project"
    );
    assert!(fixture.state.peak.load(Ordering::SeqCst) <= 4);
    assert_eq!(fixture.state.peak_checks.load(Ordering::SeqCst), 2);
    assert_eq!(
        jira.options("OPS", Purpose::Implementation, None, "")
            .await
            .unwrap()
            .assignees
            .len(),
        2,
        "negative checks count too"
    );
    assert_eq!(
        fixture
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.ends_with("/myself"))
            .count(),
        1
    );
    assert_eq!(
        done.board_revision,
        cache.read().await.unwrap().board.revision
    );
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn completed_project_checks_can_wait_for_metadata_and_options_can_commit_before_cards() {
    let fixture = Fixture::new().await;
    fixture.state.searches.add_permits(100);
    fixture.state.sprint.add_permits(100);
    fixture.state.eligibility.add_permits(100);
    let mut c = fixture.config();
    c.upstream = config().upstream; // Actual default HTTP/item caps are both four.
    let app = App::build(c).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let checked = status(&app.sync, |s| {
        let p = &s.jira_options.as_ref().unwrap().progress;
        !p.projects.is_empty() && p.projects.iter().all(|r| r.phase == SourcePhase::Completed)
    })
    .await;
    assert_eq!(
        checked.jira_options.as_ref().unwrap().progress.phase,
        SourcePhase::Discovering
    );
    assert!(!checked.jira_options.as_ref().unwrap().ready);
    assert_eq!(checked.progress[0].phase, SourcePhase::Fetching);
    assert_eq!(fixture.check_count(), 6);
    assert_eq!(fixture.state.peak_checks.load(Ordering::SeqCst), 1);
    fixture.state.discovery.add_permits(1);
    let options_first = status(&app.sync, |s| {
        s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Completed
    })
    .await;
    assert_eq!(options_first.state, "syncing");
    assert_eq!(options_first.progress[0].phase, SourcePhase::Fetching);
    assert!(options_first.last_success_at.is_none());
    let snapshot = app.cache.read().await.unwrap();
    assert_eq!(snapshot.board.revision, options_first.board_revision);
    assert!(snapshot.board.issues.is_empty() && snapshot.board.sprints.is_empty());
    assert_eq!(snapshot.jira_choices.unwrap().sprints[0].id, "1");
    fixture.state.hydration.add_permits(1);
    let done = status(&app.sync, |s| s.state == "idle").await;
    assert!(done.last_success_at.is_some());
    assert!(done.board_revision > options_first.board_revision);
    assert!(fixture.state.peak.load(Ordering::SeqCst) <= 4);
    {
        let requests = fixture.state.requests.lock().unwrap();
        for suffix in ["/myself", "/field", "/sprint"] {
            assert_eq!(
                requests.iter().filter(|r| r.ends_with(suffix)).count(),
                1,
                "shared {suffix} read"
            );
        }
    }
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn eligibility_and_http_caps_one_two_and_default_four_complete_without_deadlock() {
    for (http_cap, item_cap) in [(1, 1), (1, 2), (2, 1), (2, 2), (4, 4)] {
        let mut fixture = Fixture::new().await;
        fixture.open_tickets();
        fixture.state.discovery.add_permits(100);
        if http_cap == 1 {
            // A held response body occupies the sole transport slot. Let these
            // checks finish so the other reads can naturally serialize.
            fixture.state.eligibility.add_permits(100);
        }
        let mut c = fixture.config();
        c.upstream.max_concurrent_requests = http_cap;
        c.upstream.max_concurrent_items = item_cap;
        c.forges = vec![forge("github", "https://forge.invalid")];
        let jira = Jira::new(c.jira.as_ref().unwrap(), &c.upstream)
            .await
            .unwrap();
        let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
        let gate = Arc::new(Semaphore::new(0));
        let sync = Syncer::new(
            cache,
            vec![Arc::new(GateForge(gate.clone()))],
            Some(jira),
            3600,
        );
        let cancel = CancellationToken::new();
        let worker = sync.spawn(cancel.clone());
        fixture.until_event("check:OPS:me").await;
        status(&sync, |s| s.progress[0].phase == SourcePhase::Completed).await;
        if http_cap > 1 {
            assert_eq!(fixture.check_count(), 1);
        }
        assert_eq!(fixture.state.peak_checks.load(Ordering::SeqCst), 1);
        gate.add_permits(1);
        status(&sync, |s| {
            s.progress.iter().all(|p| p.phase == SourcePhase::Completed)
        })
        .await;
        let expected = http_cap.min(item_cap);
        while http_cap > 1 && fixture.check_count() < expected {
            fixture.event().await;
        }
        if http_cap > 1 {
            assert_eq!(fixture.check_count(), expected);
        }
        fixture.state.eligibility.add_permits(100);
        let done = status(&sync, |s| s.state == "idle").await;
        assert!(done.last_success_at.is_some(), "caps {http_cap}/{item_cap}");
        assert_eq!(fixture.check_count(), 6);
        assert_eq!(fixture.state.peak_checks.load(Ordering::SeqCst), expected);
        assert!(fixture.state.peak.load(Ordering::SeqCst) <= http_cap);
        cancel.cancel();
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn shared_sprint_failure_fails_both_domains_without_retry_or_partial_publication() {
    let fixture = Fixture::new().await;
    fixture.open_tickets();
    fixture.state.discovery.add_permits(100);
    fixture.state.eligibility.add_permits(100);
    fixture.state.fail_sprints.store(true, Ordering::SeqCst);
    let app = App::build(fixture.config()).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let done = status(&app.sync, |s| s.state == "idle" && s.started_at.is_some()).await;
    assert_eq!(done.progress[0].phase, SourcePhase::Failed);
    assert_eq!(
        done.jira_options.as_ref().unwrap().progress.phase,
        SourcePhase::Failed
    );
    assert!(!done.jira_options.as_ref().unwrap().ready);
    assert!(done.last_success_at.is_none());
    let snapshot = app.cache.read().await.unwrap();
    assert!(snapshot.board.issues.is_empty() && snapshot.board.sprints.is_empty());
    assert!(snapshot.jira_choices.is_none());
    assert_eq!(
        fixture
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.ends_with("/sprint"))
            .count(),
        1
    );
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn options_first_scope_change_quarantines_options_and_rejects_the_late_card_snapshot() {
    let fixture = Fixture::new().await;
    fixture.state.searches.add_permits(100);
    fixture.state.sprint.add_permits(100);
    fixture.state.discovery.add_permits(100);
    fixture.state.eligibility.add_permits(100);
    let app = App::build(fixture.config()).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let early = status(&app.sync, |s| {
        s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Completed
            && s.progress[0].phase == SourcePhase::Fetching
    })
    .await;
    fixture.state.changed_identity.store(true, Ordering::SeqCst);
    app.jira.as_ref().unwrap().identity().await.unwrap();
    let quarantined = app.sync.status().await.unwrap();
    assert!(quarantined.board_revision > early.board_revision);
    assert!(!quarantined.jira_options.as_ref().unwrap().ready);
    assert_eq!(
        quarantined.jira_options.as_ref().unwrap().progress.phase,
        SourcePhase::Failed
    );
    fixture.state.hydration.add_permits(1);
    let done = status(&app.sync, |s| s.state == "idle").await;
    assert_eq!(done.progress[0].phase, SourcePhase::Failed);
    assert!(done.last_success_at.is_none());
    let snapshot = app.cache.read().await.unwrap();
    assert!(snapshot.jira_choices.is_none() && snapshot.board.issues.is_empty());
    assert!(snapshot.board.sprints.is_empty());
    cancel.cancel();
    worker.await.unwrap();
}

struct QueuedForge {
    id: String,
    gate: Arc<Semaphore>,
    fail: bool,
}
#[async_trait]
impl Forge for QueuedForge {
    fn source_id(&self) -> &str {
        &self.id
    }
    async fn identity(&self) -> Result<Identity> {
        Ok(identity(&self.id, Kind::Github, "1"))
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        progress.fetching(Some(0));
        self.gate.acquire().await.unwrap().forget();
        if self.fail {
            Err(pullboard::error::Error::upstream())
        } else {
            Ok(SourceData::default())
        }
    }
}

#[tokio::test]
async fn queued_sources_count_until_their_card_attempt_terminates_including_failure() {
    let mut fixture = Fixture::new().await;
    fixture.open_tickets();
    fixture.state.discovery.add_permits(100);
    let mut c = fixture.config();
    let gate = Arc::new(Semaphore::new(0));
    let last = Arc::new(Semaphore::new(0));
    let mut forges: Vec<Arc<dyn Forge>> = vec![];
    for index in 0..4 {
        let mut f = forge("github", "https://forge.invalid");
        f.id = format!("forge-{index}");
        forges.push(Arc::new(QueuedForge {
            id: f.id.clone(),
            gate: if index == 3 {
                last.clone()
            } else {
                gate.clone()
            },
            fail: index == 3,
        }));
        c.forges.push(f);
    }
    let jira = Jira::new(c.jira.as_ref().unwrap(), &c.upstream)
        .await
        .unwrap();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let sync = Syncer::new(cache, forges, Some(jira), 3600);
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    fixture.until_event("check:OPS:me").await;
    let queued = status(&sync, |s| {
        s.progress[0].phase == SourcePhase::Completed
            && s.progress[1..4]
                .iter()
                .all(|p| p.phase == SourcePhase::Fetching)
    })
    .await;
    assert_eq!(queued.progress[4].phase, SourcePhase::Queued);
    assert_eq!(fixture.check_count(), 1);
    gate.add_permits(3);
    let last_active = status(&sync, |s| {
        s.progress[1..4]
            .iter()
            .all(|p| p.phase == SourcePhase::Completed)
            && s.progress[4].phase == SourcePhase::Fetching
    })
    .await;
    assert_eq!(last_active.state, "syncing");
    assert_eq!(fixture.check_count(), 1);
    last.add_permits(1);
    fixture.until_event("check:DEV:me").await;
    assert_eq!(
        fixture.check_count(),
        2,
        "last failed card attempt also wakes expansion"
    );
    fixture.state.eligibility.add_permits(100);
    let done = status(&sync, |s| s.state == "idle").await;
    assert_eq!(done.progress[4].phase, SourcePhase::Failed);
    assert_eq!(
        done.jira_options.as_ref().unwrap().progress.phase,
        SourcePhase::Completed
    );
    assert!(done.last_success_at.is_none());
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn metadata_failure_does_not_cancel_cards_and_ticket_failure_does_not_cancel_options() {
    for metadata_fails in [true, false] {
        let mut fixture = Fixture::new().await;
        fixture.state.searches.add_permits(100);
        fixture.state.sprint.add_permits(100);
        fixture
            .state
            .fail_metadata
            .store(metadata_fails, Ordering::SeqCst);
        fixture
            .state
            .fail_tickets
            .store(!metadata_fails, Ordering::SeqCst);
        let mut c = fixture.config();
        c.upstream.max_concurrent_requests = 4;
        let app = App::build(c).await.unwrap();
        let cancel = CancellationToken::new();
        let worker = app.sync.spawn(cancel.clone());
        let mut seen = std::collections::BTreeSet::new();
        while !seen.contains("check:OPS:me")
            || !seen.contains("/rest/api/3/issue/DEV-1/comment")
            || !seen.contains("/rest/agile/1.0/board/12/configuration")
        {
            seen.insert(fixture.event().await);
        }
        if metadata_fails {
            fixture.state.discovery.add_permits(1);
            let failed = status(&app.sync, |s| {
                s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Failed
            })
            .await;
            assert_eq!(failed.progress[0].phase, SourcePhase::Fetching);
            fixture.state.hydration.add_permits(1);
        } else {
            fixture.state.hydration.add_permits(1);
            let failed = status(&app.sync, |s| s.progress[0].phase == SourcePhase::Failed).await;
            assert_eq!(
                failed.jira_options.as_ref().unwrap().progress.phase,
                SourcePhase::Discovering
            );
            fixture.state.discovery.add_permits(1);
            fixture.state.eligibility.add_permits(100);
        }
        let done = status(&app.sync, |s| s.state == "idle").await;
        assert!(done.last_success_at.is_none());
        assert_eq!(
            done.sources[0].status,
            if metadata_fails { "ok" } else { "error" }
        );
        assert_eq!(done.jira_options.as_ref().unwrap().ready, !metadata_fails);
        assert_eq!(
            done.jira_options.as_ref().unwrap().health.status,
            if metadata_fails { "error" } else { "ok" }
        );
        let snapshot = app.cache.read().await.unwrap();
        assert_eq!(snapshot.board.issues.len(), usize::from(metadata_fails));
        assert_eq!(snapshot.jira_choices.is_some(), !metadata_fails);
        cancel.cancel();
        worker.await.unwrap();
        let requests = fixture.check_count();
        fixture.state.eligibility.add_permits(100);
        tokio::task::yield_now().await;
        assert_eq!(
            fixture.check_count(),
            requests,
            "failed options own and cancel their checks"
        );
    }
}

#[tokio::test]
async fn failed_eligibility_retains_its_own_sprints_and_health_but_never_rolls_back_fresh_cards() {
    for initial_failure in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture.open_tickets();
        fixture.state.discovery.add_permits(100);
        let app = App::build(fixture.config()).await.unwrap();
        let cancel = CancellationToken::new();
        let worker = app.sync.spawn(cancel.clone());
        fixture.until_event("check:OPS:me").await;
        let old = if initial_failure {
            None
        } else {
            fixture.state.eligibility.add_permits(6);
            status(&app.sync, |s| s.state == "idle").await;
            let snapshot = app.cache.read().await.unwrap();
            fixture.state.sprint_id.store(2, Ordering::SeqCst);
            app.sync.request().await.unwrap();
            fixture.until_event("check:OPS:me").await;
            Some(snapshot)
        };
        let fetching = status(&app.sync, |s| {
            s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Fetching
                && s.progress[0].phase == SourcePhase::Completed
        })
        .await;
        if initial_failure {
            use tower::ServiceExt;
            let response = pullboard::api::router(app.clone())
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/api/context")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let context: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap(),
            )
            .unwrap();
            validate_contract(&context, "Context");
            assert_eq!(context["current_project"]["key"], "OPS");
            assert_eq!(context["current_project"]["name"], "Operations");
            assert_eq!(context["current_project"]["issue_types"], json!([]));
            assert_eq!(context["current_sprints"][0]["id"], "1");
        }
        assert_eq!(
            fetching.jira_options.as_ref().unwrap().ready,
            !initial_failure
        );
        if let Some(old) = &old {
            let board = app.cache.read().await.unwrap().board;
            assert_eq!(board.sprints[0].id, "2");
            assert_eq!(
                app.jira
                    .as_ref()
                    .unwrap()
                    .options("OPS", Purpose::Review, None, "")
                    .await
                    .unwrap()
                    .sprints[0]
                    .id,
                "1"
            );
            assert!(board.sources[0].last_success_at > old.board.sources[0].last_success_at);
            let mut request = create();
            request.sprint_id = Some("2".into());
            let jira = app.jira.as_ref().unwrap();
            assert_eq!(
                jira.create(&request, &pr(), &board.sprints, true)
                    .await
                    .unwrap_err()
                    .code,
                "invalid_choice",
                "new Board sprints cannot authorize an old options generation"
            );
            request.sprint_id = Some("1".into());
            assert_eq!(
                jira.create(&request, &pr(), &board.sprints, true)
                    .await
                    .unwrap()
                    .outcome,
                "simulated"
            );
        }
        // Subsequent checks fail; already running checks may finish normally.
        fixture.state.fail_checks.store(true, Ordering::SeqCst);
        fixture.state.eligibility.add_permits(20);
        let failed = status(&app.sync, |s| s.state == "idle").await;
        assert_eq!(failed.sources[0].status, "ok");
        assert_eq!(failed.progress[0].phase, SourcePhase::Completed);
        let options = failed.jira_options.unwrap();
        assert_eq!(options.health.status, "error");
        assert_eq!(options.progress.phase, SourcePhase::Failed);
        assert_eq!(options.ready, !initial_failure);
        assert!(
            options
                .progress
                .projects
                .iter()
                .all(|r| matches!(r.phase, SourcePhase::Failed | SourcePhase::Completed))
        );
        let after = app.cache.read().await.unwrap();
        assert_eq!(after.board.issues.len(), 1);
        if let Some(old) = old {
            assert_eq!(
                serde_json::to_value(&after.jira_choices).unwrap(),
                serde_json::to_value(&old.jira_choices).unwrap()
            );
            assert_eq!(
                options.health.last_success_at,
                old.jira_options_health.unwrap().last_success_at
            );
            assert_eq!(failed.last_success_at, fetching.last_success_at);
        } else {
            assert!(after.jira_choices.is_none());
            assert!(options.health.last_success_at.is_none());
            assert_eq!(
                app.jira
                    .as_ref()
                    .unwrap()
                    .options("OPS", Purpose::Review, None, "")
                    .await
                    .unwrap_err()
                    .code,
                "jira_discovery_failed"
            );
        }
        cancel.cancel();
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn groups_remain_indeterminate_until_all_pages_are_verified_and_reset_on_cancel() {
    let mut fixture = Fixture::new().await;
    fixture.open_tickets();
    fixture.state.discovery.add_permits(10);
    fixture.state.block_groups.store(true, Ordering::SeqCst);
    let app = App::build(fixture.config()).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let mut first_pages = std::collections::BTreeSet::new();
    while first_pages.len() < 2 {
        let event = fixture.event().await;
        if event.starts_with("group:") {
            first_pages.insert(event);
        }
    }
    assert_eq!(
        first_pages,
        ["group:Group A:0".to_string(), "group:Group B:0".to_string()]
            .into_iter()
            .collect()
    );
    let discovering = app.sync.status().await.unwrap();
    validate_contract(&serde_json::to_value(&discovering).unwrap(), "SyncStatus");
    assert!(
        discovering
            .jira_options
            .unwrap()
            .progress
            .groups
            .iter()
            .all(|r| r.phase == SourcePhase::Discovering
                && r.completed_items == 0
                && r.total_items.is_none())
    );
    fixture.state.groups.add_permits(2);
    let mut second_pages = std::collections::BTreeSet::new();
    while second_pages.len() < 2 {
        let event = fixture.event().await;
        if event.starts_with("group:") {
            assert!(event.ends_with(":1"));
            second_pages.insert(event);
        }
    }
    assert!(
        app.sync
            .status()
            .await
            .unwrap()
            .jira_options
            .unwrap()
            .progress
            .groups
            .iter()
            .all(|r| r.completed_items == 0 && r.total_items.is_none())
    );
    fixture.state.groups.add_permits(1);
    let partial = status(&app.sync, |s| {
        s.jira_options
            .as_ref()
            .unwrap()
            .progress
            .groups
            .iter()
            .filter(|r| r.phase == SourcePhase::Completed)
            .count()
            == 1
    })
    .await;
    assert_eq!(fixture.check_count(), 0);
    assert!(!partial.jira_options.unwrap().ready);
    cancel.cancel();
    worker.await.unwrap();
    let cancelled = app.sync.status().await.unwrap();
    validate_contract(&serde_json::to_value(&cancelled).unwrap(), "SyncStatus");
    let options = cancelled.jira_options.unwrap();
    assert_eq!(options.progress.phase, SourcePhase::Failed);
    assert!(
        options
            .progress
            .groups
            .iter()
            .all(|r| matches!(r.phase, SourcePhase::Completed | SourcePhase::Failed))
    );
    assert!(app.cache.read().await.unwrap().jira_choices.is_none());
    let reset = app.sync.request().await.unwrap().jira_options.unwrap();
    assert_eq!(reset.progress.phase, SourcePhase::Queued);
    assert!(reset.progress.started_at.is_none());
    assert_eq!(reset.progress.elapsed_ms, 0);
    assert!(reset.progress.groups.is_empty() && reset.progress.projects.is_empty());
}

#[tokio::test]
async fn options_backoff_is_independent_persisted_and_skipped_resets_never_bypass_the_shared_deadline()
 {
    let fixture = Fixture::new().await;
    fixture.open_tickets();
    fixture.state.discovery.add_permits(100);
    fixture.state.block_groups.store(true, Ordering::SeqCst);
    fixture
        .state
        .rate_limit_checks
        .store(true, Ordering::SeqCst);
    let app = App::build(fixture.config()).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    app.sync.request().await.unwrap();
    status(&app.sync, |s| s.progress[0].phase == SourcePhase::Completed).await;
    fixture.state.groups.add_permits(100);
    let first = status(&app.sync, |s| s.state == "idle").await;
    assert_eq!(first.sources[0].status, "ok");
    let options = first.jira_options.unwrap();
    assert_eq!(options.health.status, "backoff");
    assert_eq!(options.progress.phase, SourcePhase::Backoff);
    assert!(options.progress.started_at.is_some() && options.progress.finished_at.is_some());
    assert!(
        options
            .progress
            .projects
            .iter()
            .all(|r| r.phase == SourcePhase::Backoff && r.completed_items == 0)
    );
    assert_eq!(
        app.cache
            .read()
            .await
            .unwrap()
            .jira_options_health
            .unwrap()
            .retry_at,
        options.health.retry_at
    );
    let requests = fixture.state.requests.lock().unwrap().len();
    let reset = app.sync.request().await.unwrap().jira_options.unwrap();
    assert_eq!(reset.progress.phase, SourcePhase::Backoff);
    assert!(reset.progress.started_at.is_none() && reset.progress.finished_at.is_none());
    assert_eq!(reset.progress.elapsed_ms, 0);
    assert!(reset.progress.groups.is_empty() && reset.progress.projects.is_empty());
    let skipped = status(&app.sync, |s| s.state == "idle").await;
    assert_eq!(
        skipped.jira_options.unwrap().health.last_attempt_at,
        options.health.last_attempt_at
    );
    assert_eq!(skipped.sources[0].status, "backoff");
    assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
    assert!(skipped.last_success_at.is_none());
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn quarantine_rejects_delayed_eligibility_and_cancellation_owns_the_blocked_batch() {
    for change_identity in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture.open_tickets();
        fixture.state.discovery.add_permits(100);
        let app = App::build(fixture.config()).await.unwrap();
        let cancel = CancellationToken::new();
        let worker = app.sync.spawn(cancel.clone());
        fixture.until_event("check:OPS:me").await;
        let active = status(&app.sync, |s| {
            s.jira_options.as_ref().unwrap().progress.phase == SourcePhase::Fetching
                && s.progress[0].phase == SourcePhase::Completed
        })
        .await;
        if change_identity {
            fixture.state.changed_identity.store(true, Ordering::SeqCst);
            app.jira.as_ref().unwrap().identity().await.unwrap();
            let quarantined = app.sync.status().await.unwrap();
            assert!(quarantined.board_revision > active.board_revision);
            assert!(!quarantined.jira_options.as_ref().unwrap().ready);
            assert!(
                quarantined
                    .jira_options
                    .as_ref()
                    .unwrap()
                    .health
                    .last_success_at
                    .is_none()
            );
            assert!(app.cache.read().await.unwrap().board.issues.is_empty());
            fixture.state.eligibility.add_permits(20);
            let failed = status(&app.sync, |s| s.state == "idle").await;
            assert_eq!(
                failed.jira_options.unwrap().progress.phase,
                SourcePhase::Failed
            );
            assert!(app.cache.read().await.unwrap().jira_choices.is_none());
            assert!(app.cache.read().await.unwrap().board.issues.is_empty());
        }
        cancel.cancel();
        tokio::time::timeout(DEADLINE, worker)
            .await
            .unwrap()
            .unwrap();
        let finished = app.sync.status().await.unwrap();
        validate_contract(&serde_json::to_value(&finished).unwrap(), "SyncStatus");
        assert_eq!(finished.state, "idle");
        assert_eq!(
            finished.jira_options.unwrap().progress.phase,
            SourcePhase::Failed
        );
        assert!(app.cache.read().await.unwrap().jira_choices.is_none());
        let requests = fixture.check_count();
        fixture.state.eligibility.add_permits(20);
        tokio::task::yield_now().await;
        assert_eq!(
            fixture.check_count(),
            requests,
            "no detached queue continues after shutdown"
        );
    }
}
