//! Real HTTP body gates prove hydration overlap without elapsed-time assertions.
mod common;

use axum::{Router, body::Body, extract::State, response::Response};
use common::*;
use futures_util::stream;
use pullboard::{
    cache::{Cache, SourceData, SourceUpdate},
    config::Upstream,
    error::Result,
    forge::{self, Forge},
    jira::Jira,
    model::Identity,
    sync::Progress,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};
use wiremock::MockServer;

const ITEMS: usize = 5;
const DEADLINE: Duration = Duration::from_secs(5);

struct Fixture {
    kind: &'static str,
    metadata: String,
    gates: Vec<Semaphore>,
    hits: mpsc::UnboundedSender<(usize, String)>,
    requests: Mutex<Vec<String>>,
    finished: Mutex<BTreeMap<usize, usize>>,
    active_items: Mutex<BTreeSet<usize>>,
    peak_items: AtomicUsize,
    active_reads: AtomicUsize,
    peak_reads: AtomicUsize,
    fail: AtomicUsize,
}
impl Fixture {
    fn initial_reads(&self) -> usize {
        match self.kind {
            "jira" => 3,
            "gitlab" => 2,
            _ => 1, // Pull details must precede history reads.
        }
    }
    fn total_reads(&self) -> usize {
        match self.kind {
            "jira" => 3,
            "gitlab" => 2,
            _ => 4,
        }
    }
    fn release(&self, item: usize, count: usize) {
        self.gates[item].add_permits(count);
    }
    fn release_all(&self) {
        for item in 1..=ITEMS {
            self.release(item, 100);
        }
    }
    fn item_requests(&self, item: usize) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|path| item_path(self.kind, path).is_some_and(|(id, _)| id == item))
            .count()
    }
}

struct ActiveRead(Arc<Fixture>);
impl Drop for ActiveRead {
    fn drop(&mut self) {
        self.0.active_reads.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Server {
    base: String,
    state: Arc<Fixture>,
    hits: mpsc::UnboundedReceiver<(usize, String)>,
    task: tokio::task::JoinHandle<()>,
    _metadata: MockServer,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.release_all();
        self.task.abort();
    }
}

fn item_path(kind: &str, path: &str) -> Option<(usize, String)> {
    let parts: Vec<_> = path.split('/').collect();
    let item = if kind == "jira" {
        path.strip_prefix("/rest/api/3/issue/DEV-")?
            .split('/')
            .next()?
            .parse()
            .ok()?
    } else if kind == "gitlab" {
        path.strip_prefix("/api/v4/projects/10/merge_requests/")?
            .split('/')
            .next()?
            .parse()
            .ok()?
    } else {
        if !path.starts_with("/api/v1/repos/acme/widget/") {
            return None;
        }
        parts.get(7)?.parse().ok()?
    };
    Some((item, parts.last()?.to_string()))
}

fn candidate(kind: &str, item: usize) -> Value {
    if kind == "jira" {
        json!({"id":item.to_string(),"key":format!("DEV-{item}"),"fields":{
            "summary":format!("Item {item}"),"description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"mention","attrs":{"id":"me"}}]}]},
            "project":{"key":"DEV"},"issuetype":{"name":"Task"},"status":{"name":"Open"},
            "reporter":jira_user("other"),"assignee":jira_user("me"),
            "created":"2026-01-01T00:00:00Z","updated":"2026-01-02T00:00:00Z"
        }})
    } else if kind == "gitlab" {
        json!({"iid":item,"project_id":10,"title":format!("Item {item}"),"description":"",
            "web_url":format!("https://gitlab.example/acme/widget/-/merge_requests/{item}"),
            "author":gitlab_user(1,"me"),"created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-02T00:00:00Z","state":"opened"})
    } else {
        json!({"number":item,"repository":{"full_name":"acme/widget"}})
    }
}

fn item_body(kind: &str, item: usize, endpoint: &str) -> Value {
    match (kind, endpoint) {
        ("jira", "comment") => json!({"comments":[],"total":0,"startAt":0,"maxResults":100}),
        ("jira", "changelog") => json!({"values":[],"isLast":true,"startAt":0,"maxResults":100}),
        ("jira", "remotelink") => json!([]),
        ("gitlab", _) => json!([]),
        (_, "comments" | "reviews" | "commits") => json!([]),
        _ => json!({"number":item,"title":format!("Item {item}"),"body":"",
            "html_url":format!("https://forge.example/acme/widget/pulls/{item}"),
            "user":forgejo_user(1,"me"),"created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-02T00:00:00Z"}),
    }
}

async fn handler(State(state): State<Arc<Fixture>>, request: axum::extract::Request) -> Response {
    assert_eq!(request.method(), "GET", "refresh must never write");
    let uri = request.uri().to_string();
    let path = request.uri().path().to_owned();
    state.requests.lock().unwrap().push(path.clone());
    if let Some((item, endpoint)) = item_path(state.kind, &path) {
        {
            let mut active = state.active_items.lock().unwrap();
            active.insert(item);
            state.peak_items.fetch_max(active.len(), Ordering::SeqCst);
        }
        let active = state.active_reads.fetch_add(1, Ordering::SeqCst) + 1;
        state.peak_reads.fetch_max(active, Ordering::SeqCst);
        state.hits.send((item, endpoint.clone())).unwrap();
        let guard = ActiveRead(Arc::clone(&state));
        return Response::new(Body::from_stream(stream::once(async move {
            state.gates[item].acquire().await.unwrap().forget();
            let body = if state.fail.load(Ordering::SeqCst) == item {
                // A required read with a malformed success body fails all SDKs in
                // the same way, after the test explicitly releases this gate.
                "private upstream payload".to_owned()
            } else {
                item_body(state.kind, item, &endpoint).to_string()
            };
            {
                let mut finished = state.finished.lock().unwrap();
                let count = finished.entry(item).or_default();
                *count += 1;
                if *count == state.total_reads() {
                    state.active_items.lock().unwrap().remove(&item);
                }
            }
            drop(guard);
            Ok::<_, Infallible>(body)
        })));
    }
    let body = match path.as_str() {
        "/api/v4/issues" => json!([]),
        "/api/v1/repos/issues/search" if uri.contains("type=issues") => json!([]),
        "/api/v4/user" => gitlab_user(1, "me"),
        "/api/v1/user" => forgejo_user(1, "me"),
        "/rest/api/3/search/jql" => {
            json!({"issues": (1..=ITEMS).rev().map(|i| candidate("jira", i)).collect::<Vec<_>>(),"isLast":true})
        }
        "/api/v4/merge_requests" | "/api/v1/repos/issues/search" => {
            // Every account search returns duplicate candidates in reversed order.
            let mut candidates: Vec<_> = (1..=ITEMS)
                .rev()
                .map(|i| candidate(state.kind, i))
                .collect();
            candidates.push(candidate(state.kind, 1));
            json!(candidates)
        }
        "/api/v4/todos" => json!([]),
        "/api/v4/projects/10" => json!({"path_with_namespace":"acme/widget"}),
        _ if state.kind == "jira" => {
            // Reuse the existing complete, read-only Jira metadata fixture.
            let response = reqwest::get(format!("{}{uri}", state.metadata))
                .await
                .unwrap();
            assert!(response.status().is_success(), "missing fixture: {uri}");
            response.json::<Value>().await.unwrap()
        }
        _ => panic!("unexpected adapter request: {uri}"),
    };
    Response::new(Body::from(body.to_string()))
}

async fn server(kind: &'static str) -> Server {
    let metadata = MockServer::start().await;
    if kind == "jira" {
        mock_jira(&metadata).await;
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let state = Arc::new(Fixture {
        kind,
        metadata: metadata.uri(),
        gates: (0..=ITEMS).map(|_| Semaphore::new(0)).collect(),
        hits: tx,
        requests: Mutex::new(vec![]),
        finished: Mutex::new(BTreeMap::new()),
        active_items: Mutex::new(BTreeSet::new()),
        peak_items: AtomicUsize::new(0),
        active_reads: AtomicUsize::new(0),
        peak_reads: AtomicUsize::new(0),
        fail: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(handler)
        .with_state(Arc::clone(&state));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base,
        state,
        hits: rx,
        task,
        _metadata: metadata,
    }
}

enum Adapter {
    Jira(Arc<Jira>),
    Forge(Arc<dyn Forge>),
}
impl Adapter {
    async fn new(server: &Server, limits: &Upstream) -> Arc<Self> {
        Arc::new(if server.state.kind == "jira" {
            Self::Jira(Jira::new(&jira(&server.base), limits).await.unwrap())
        } else {
            Self::Forge(
                forge::build(&forge(server.state.kind, &server.base), limits)
                    .await
                    .unwrap(),
            )
        })
    }
    async fn discover(&self, progress: Progress) -> Result<SourceData> {
        match self {
            Self::Jira(jira) => jira.discover_with_progress(progress).await,
            Self::Forge(forge) => forge.discover_with_progress(progress).await,
        }
    }
    async fn identity(&self) -> Identity {
        match self {
            Self::Jira(jira) => jira.identity().await.unwrap(),
            Self::Forge(forge) => forge.identity().await.unwrap(),
        }
    }
}

async fn hits(server: &mut Server, expected: BTreeMap<usize, usize>) {
    let mut actual = BTreeMap::new();
    for _ in 0..expected.values().sum() {
        let (item, _) = tokio::time::timeout(DEADLINE, server.hits.recv())
            .await
            .unwrap()
            .unwrap();
        *actual.entry(item).or_default() += 1;
    }
    assert_eq!(actual, expected);
}

async fn complete(server: &mut Server, item: usize) {
    server.state.release(item, server.state.initial_reads());
    if matches!(server.state.kind, "forgejo" | "codeberg") {
        hits(server, BTreeMap::from([(item, 3)])).await;
        server.state.release(item, 3);
    }
}

async fn reads_cancelled(state: &Fixture) {
    tokio::time::timeout(DEADLINE, async {
        while state.active_reads.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn bounded_ordered_success(kind: &'static str, cap: usize) {
    let mut server = server(kind).await;
    let limits = Upstream {
        max_concurrent_items: cap,
        max_concurrent_requests: 16,
        ..Default::default()
    };
    let adapter = Adapter::new(&server, &limits).await;
    let progress = Progress::recording(if kind == "jira" { "jira" } else { "test-forge" });
    let task = tokio::spawn({
        let progress = progress.clone();
        async move { adapter.discover(progress).await }
    });
    let initial = server.state.initial_reads();
    hits(&mut server, (1..=cap).map(|i| (i, initial)).collect()).await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 0);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(ITEMS as u64));
    // Finish the later candidate while the first remains blocked. The next slot
    // must start without waiting for discovery-order aggregation.
    complete(&mut server, cap).await;
    hits(&mut server, BTreeMap::from([(cap + 1, initial)])).await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    server.state.release_all();
    let data = tokio::time::timeout(DEADLINE, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, ITEMS as u64);
    assert_eq!(server.state.peak_items.load(Ordering::SeqCst), cap);
    let numbers: Vec<_> = if kind == "jira" {
        data.issues
            .iter()
            .map(|issue| {
                issue
                    .key
                    .rsplit_once('-')
                    .unwrap()
                    .1
                    .parse::<usize>()
                    .unwrap()
            })
            .collect()
    } else {
        data.pull_requests
            .iter()
            .map(|pr| usize::try_from(pr.number).unwrap())
            .collect()
    };
    assert_eq!(numbers, (1..=ITEMS).collect::<Vec<_>>());
    for item in 1..=ITEMS {
        assert_eq!(server.state.item_requests(item), server.state.total_reads());
    }
}

async fn fail_closed_cancels_siblings(kind: &'static str) {
    let mut server = server(kind).await;
    let limits = Upstream {
        max_concurrent_items: 2,
        max_concurrent_requests: 16,
        ..Default::default()
    };
    let adapter = Adapter::new(&server, &limits).await;
    let mut config = config();
    let source_id = if kind == "jira" {
        config.jira = Some(jira(&server.base));
        "jira"
    } else {
        config.forges = vec![forge(kind, &server.base)];
        "test-forge"
    };
    let cache = Cache::open(&config.server.database, &config).await.unwrap();
    cache.verify(adapter.identity().await).await.unwrap();
    let old = if kind == "jira" {
        SourceData {
            issues: vec![issue()],
            ..Default::default()
        }
    } else {
        let mut pr = pr();
        pr.forge = config.forges[0].kind;
        SourceData {
            pull_requests: vec![pr],
            ..Default::default()
        }
    };
    let before = cache
        .publish(vec![SourceUpdate {
            source_id: source_id.into(),
            result: Ok(old),
        }])
        .await
        .unwrap();
    let progress = Progress::recording(source_id);
    let task = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        let progress = progress.clone();
        async move { adapter.discover(progress).await }
    });
    let initial = server.state.initial_reads();
    hits(&mut server, BTreeMap::from([(1, initial), (2, initial)])).await;
    complete(&mut server, 2).await;
    hits(&mut server, BTreeMap::from([(3, initial)])).await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    server.state.fail.store(3, Ordering::SeqCst);
    server.state.release(3, 1);
    let error = tokio::time::timeout(DEADLINE, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(!error.message.contains("private upstream payload"));
    reads_cancelled(&server.state).await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(ITEMS as u64));
    assert_eq!(server.state.item_requests(4), 0);
    assert_eq!(server.state.item_requests(5), 0);
    let after = cache
        .publish(vec![SourceUpdate {
            source_id: source_id.into(),
            result: Err(error),
        }])
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&before.board.issues).unwrap(),
        serde_json::to_value(&after.board.issues).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.board.pull_requests).unwrap(),
        serde_json::to_value(&after.board.pull_requests).unwrap()
    );
    assert_eq!(
        before.board.sources[0].last_success_at,
        after.board.sources[0].last_success_at
    );
    assert_eq!(after.board.sources[0].status, "error");
    // No detached item tasks survive a failed source or later mutate progress.
    server.state.release_all();
    adapter.identity().await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert_eq!(server.state.item_requests(4), 0);
}

async fn cancelled_attempt_stops_reads(kind: &'static str) {
    let mut server = server(kind).await;
    let limits = Upstream {
        max_concurrent_items: 2,
        max_concurrent_requests: 16,
        ..Default::default()
    };
    let adapter = Adapter::new(&server, &limits).await;
    let progress = Progress::recording("source");
    let task = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        let progress = progress.clone();
        async move { adapter.discover(progress).await }
    });
    let initial = server.state.initial_reads();
    hits(&mut server, BTreeMap::from([(1, initial), (2, initial)])).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    reads_cancelled(&server.state).await;
    server.state.release_all();
    adapter.identity().await;
    assert_eq!(progress.snapshot().unwrap().completed_items, 0);
    assert_eq!(server.state.item_requests(3), 0);
}

#[tokio::test]
async fn jira_hydration_is_bounded_ordered_fail_closed_and_cancellable() {
    bounded_ordered_success("jira", 1).await;
    bounded_ordered_success("jira", 2).await;
    fail_closed_cancels_siblings("jira").await;
    cancelled_attempt_stops_reads("jira").await;
}

#[tokio::test]
async fn gitlab_hydration_is_bounded_ordered_fail_closed_and_cancellable() {
    bounded_ordered_success("gitlab", 1).await;
    bounded_ordered_success("gitlab", 2).await;
    fail_closed_cancels_siblings("gitlab").await;
    cancelled_attempt_stops_reads("gitlab").await;
}

#[tokio::test]
async fn forgejo_and_codeberg_hydration_is_bounded_ordered_fail_closed_and_cancellable() {
    for kind in ["forgejo", "codeberg"] {
        bounded_ordered_success(kind, 1).await;
        bounded_ordered_success(kind, 2).await;
        fail_closed_cancels_siblings(kind).await;
        cancelled_attempt_stops_reads(kind).await;
    }
}

#[tokio::test]
async fn independent_history_reads_obey_the_shared_default_four_request_budget() {
    for kind in ["jira", "gitlab", "forgejo", "codeberg"] {
        let mut server = server(kind).await;
        let adapter = Adapter::new(&server, &Upstream::default()).await;
        let progress = Progress::recording("source");
        let task = tokio::spawn({
            let progress = progress.clone();
            async move { adapter.discover(progress).await }
        });
        // Four real bodies must be pending before anything is released. For
        // Forgejo these are the detail reads; release them to gate history too.
        let mut seen = BTreeSet::new();
        for _ in 0..4 {
            let (item, _) = tokio::time::timeout(DEADLINE, server.hits.recv())
                .await
                .unwrap()
                .unwrap();
            seen.insert(item);
        }
        assert_eq!(server.state.active_reads.load(Ordering::SeqCst), 4);
        assert_eq!(progress.snapshot().unwrap().completed_items, 0);
        if matches!(kind, "forgejo" | "codeberg") {
            assert_eq!(seen, BTreeSet::from([1, 2, 3, 4]));
            for item in seen {
                server.state.release(item, 1);
            }
            for _ in 0..4 {
                let (_, endpoint) = tokio::time::timeout(DEADLINE, server.hits.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(
                    endpoint.as_str(),
                    "comments" | "reviews" | "commits"
                ));
            }
            assert_eq!(server.state.active_reads.load(Ordering::SeqCst), 4);
            assert_eq!(progress.snapshot().unwrap().completed_items, 0);
        }
        server.state.release_all();
        tokio::time::timeout(DEADLINE, task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(progress.snapshot().unwrap().completed_items, ITEMS as u64);
        assert_eq!(server.state.peak_reads.load(Ordering::SeqCst), 4);
    }
}
