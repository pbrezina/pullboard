//! Request-level regressions independent of adapter hydration/progress.
mod common;

use axum::{Router, body::Body, extract::State, response::Response};
use common::{forge, jira};
use futures_util::{future::join_all, stream};
use pullboard::{config::Upstream, jira::Jira, transport::ReadTransport};
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[derive(Default)]
struct Counts {
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
}
struct Active(Arc<Counts>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
struct StreamingServer {
    base: String,
    counts: Arc<Counts>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for StreamingServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn streaming_server() -> StreamingServer {
    async fn handler(State(counts): State<Arc<Counts>>) -> Response {
        counts.calls.fetch_add(1, Ordering::SeqCst);
        let active = counts.active.fetch_add(1, Ordering::SeqCst) + 1;
        counts.peak.fetch_max(active, Ordering::SeqCst);
        let guard = Active(counts);
        Response::new(Body::from_stream(stream::once(async move {
            // Headers arrive immediately; only body consumption is delayed.
            tokio::time::sleep(Duration::from_millis(40)).await;
            drop(guard);
            Ok::<_, Infallible>(
                r#"{"accountId":"me","displayName":"Me","active":true,"accountType":"atlassian"}"#,
            )
        })))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let counts = Arc::new(Counts::default());
    let app = Router::new()
        .fallback(handler)
        .with_state(Arc::clone(&counts));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    StreamingServer { base, counts, task }
}

#[tokio::test]
async fn concurrency_is_shared_across_sdks_instances_and_jira_through_body_read() {
    let server = streaming_server().await;
    let limits = Upstream {
        max_concurrent_requests: 2,
        ..Upstream::default()
    };
    let mut transports = Vec::new();
    for kind in ["github", "github", "gitlab", "forgejo"] {
        transports.push(
            ReadTransport::new(&forge(kind, &server.base), &limits)
                .await
                .unwrap(),
        );
    }
    let jira = Jira::new(&jira(&server.base), &limits).await.unwrap();
    let requests = (0..12).map(|i| {
        let transport = Arc::clone(&transports[i % transports.len()]);
        async move { transport.get::<Value>("test", &[]).await.unwrap() }
    });
    let (values, identity) = tokio::join!(join_all(requests), jira.identity());
    assert_eq!(values.len(), 12);
    identity.unwrap();
    assert_eq!(server.counts.calls.load(Ordering::SeqCst), 13);
    assert_eq!(server.counts.peak.load(Ordering::SeqCst), 2);
    assert_eq!(server.counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(transports[0].limits().max_concurrent_requests, 2);
}

#[tokio::test]
async fn queued_requests_on_other_instances_stop_after_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "120")
                .set_delay(Duration::from_millis(50)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let limits = Upstream {
        max_concurrent_requests: 1,
        ..Upstream::default()
    };
    let a = ReadTransport::new(&forge("forgejo", &server.uri()), &limits)
        .await
        .unwrap();
    let b = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let results = join_all((0..12).map(|i| {
        let transport = if i % 2 == 0 { &a } else { &b };
        async move { transport.get::<Value>("test", &[]).await.unwrap_err() }
    }))
    .await;
    for error in results {
        assert_eq!(error.code, "upstream_backoff");
        assert!(error.retry_at.unwrap() > chrono::Utc::now() + chrono::Duration::seconds(115));
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn gitlab_oversized_429_keeps_retry_after_and_suppresses_queued_shared_origin_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "120")
                .set_body_string("private upstream error".repeat(100))
                .set_delay(Duration::from_millis(40)),
        )
        .mount(&server)
        .await;
    let limits = Upstream {
        max_concurrent_requests: 1,
        max_response_bytes: 16,
        ..Default::default()
    };
    let gitlab = ReadTransport::new(&forge("gitlab", &server.uri()), &limits)
        .await
        .unwrap();
    let github = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let jira = Jira::new(&jira(&server.uri()), &limits).await.unwrap();
    let (first, queued_forge, queued_jira) = tokio::join! {
        biased;
        gitlab.get::<Value>("issues", &[]),
        github.get::<Value>("user", &[]),
        jira.identity(),
    };
    for error in [
        first.unwrap_err(),
        queued_forge.unwrap_err(),
        queued_jira.unwrap_err(),
    ] {
        assert_eq!(error.code, "upstream_backoff");
        assert!(error.retry_at.unwrap() > chrono::Utc::now() + chrono::Duration::seconds(115));
        assert!(!error.message.contains("private upstream error"));
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn gitlab_429_headers_suppress_queued_requests_before_a_slow_failing_body_finishes() {
    struct RateStream {
        calls: AtomicUsize,
        started: tokio::sync::Semaphore,
        release: tokio::sync::Semaphore,
    }
    async fn handler(State(state): State<Arc<RateStream>>) -> Response {
        state.calls.fetch_add(1, Ordering::SeqCst);
        state.started.add_permits(1);
        Response::builder()
            .status(429)
            .header("retry-after", "120")
            .body(Body::from_stream(stream::once(async move {
                state.release.acquire().await.unwrap().forget();
                Err::<&'static str, _>(std::io::Error::other("private upstream stream failure"))
            })))
            .unwrap()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(RateStream {
        calls: AtomicUsize::new(0),
        started: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let app = Router::new().fallback(handler).with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let limits = Upstream {
        max_concurrent_requests: 1,
        ..Default::default()
    };
    let gitlab = ReadTransport::new(&forge("gitlab", &base), &limits)
        .await
        .unwrap();
    let github = ReadTransport::new(&forge("github", &base), &limits)
        .await
        .unwrap();
    let reader = tokio::spawn(async move { gitlab.get::<Value>("issues", &[]).await });
    state.started.acquire().await.unwrap().forget();
    let queued =
        tokio::time::timeout(Duration::from_secs(1), github.get::<Value>("user", &[])).await;
    state.release.add_permits(1);
    let first = reader.await.unwrap().unwrap_err();
    server.abort();
    assert_eq!(first.code, "upstream_backoff");
    assert_eq!(
        queued
            .expect("queued work must see headers before body completion")
            .unwrap_err()
            .code,
        "upstream_backoff"
    );
    assert!(first.retry_at.unwrap() > chrono::Utc::now() + chrono::Duration::seconds(115));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn graphql_200_secondary_limit_suppresses_queued_rest_and_jira_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(json!({"errors":[{"message":"You have exceeded a secondary rate limit. Please wait.","extensions":{"code":"RATE_LIMITED"}}]}))
            .set_delay(Duration::from_millis(50)))
        .expect(1).mount(&server).await;
    let limits = Upstream {
        max_concurrent_requests: 1,
        ..Upstream::default()
    };
    let a = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let b = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let jira = Jira::new(&jira(&server.uri()), &limits).await.unwrap();
    let (graphql, rest, jira_result) = tokio::join!(
        biased;
        a.query::<Value, _>("query Test { viewer { login } }", json!({})),
        b.get::<Value>("user", &[]),
        jira.identity(),
    );
    for error in [
        graphql.unwrap_err(),
        rest.unwrap_err(),
        jira_result.unwrap_err(),
    ] {
        assert_eq!(error.code, "upstream_backoff");
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn successful_exhausted_primary_headers_only_stop_their_github_bucket() {
    let server = MockServer::start().await;
    let reset = (chrono::Utc::now() + chrono::Duration::seconds(120))
        .timestamp()
        .to_string();
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items":[]}))
                .insert_header("x-ratelimit-resource", "search")
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", reset.as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":1})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"me"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let limits = Upstream::default();
    let a = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let b = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    a.get::<Value>("search/issues", &[]).await.unwrap();
    let blocked = b.get::<Value>("search/issues", &[]).await.unwrap_err();
    assert_eq!(blocked.code, "upstream_backoff");
    b.get::<Value>("user", &[]).await.unwrap();
    b.query::<Value, _>("query Test { viewer { login } }", json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn http_403_secondary_body_without_rate_headers_stops_other_buckets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(
                json!({"message":"You have triggered an abuse detection mechanism."}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let transport = ReadTransport::new(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert_eq!(
        transport
            .get::<Value>("search/issues", &[])
            .await
            .unwrap_err()
            .code,
        "upstream_backoff"
    );
    assert_eq!(
        transport.get::<Value>("user", &[]).await.unwrap_err().code,
        "upstream_backoff"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn graphql_success_rate_metadata_exhausts_only_graphql_budget() {
    let server = MockServer::start().await;
    let reset = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339();
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":{"rateLimit":{"remaining":0,"resetAt":reset},"viewer":{"login":"me"}}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":1})))
        .expect(1)
        .mount(&server)
        .await;
    let limits = Upstream::default();
    let a = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let b = ReadTransport::new(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    a.query::<Value, _>("query Test { viewer { login } }", json!({}))
        .await
        .unwrap();
    assert_eq!(
        b.query::<Value, _>("query Test { viewer { login } }", json!({}))
            .await
            .unwrap_err()
            .code,
        "upstream_backoff"
    );
    b.get::<Value>("user", &[]).await.unwrap();
}

#[tokio::test]
async fn graphql_typed_primary_rate_error_honors_reset_without_blocking_core() {
    let server = MockServer::start().await;
    let reset = (chrono::Utc::now() + chrono::Duration::seconds(120))
        .timestamp()
        .to_string();
    Mock::given(method("POST")).and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("x-ratelimit-resource", "graphql")
            .insert_header("x-ratelimit-remaining", "0")
            .insert_header("x-ratelimit-reset", reset.as_str())
            .set_body_json(json!({"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded."}]})))
        .expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":1})))
        .expect(1)
        .mount(&server)
        .await;
    let transport = ReadTransport::new(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let error = transport
        .query::<Value, _>("query Test { viewer { login } }", json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.retry_at.unwrap().timestamp().to_string(), reset);
    assert_eq!(
        transport
            .query::<Value, _>("query Test { viewer { login } }", json!({}))
            .await
            .unwrap_err()
            .code,
        "upstream_backoff"
    );
    transport.get::<Value>("user", &[]).await.unwrap();
}

#[tokio::test]
async fn rest_retries_are_bounded_and_graphql_mutations_remain_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;
    let transport = ReadTransport::new(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert_eq!(
        transport.get::<Value>("user", &[]).await.unwrap_err().code,
        "upstream_failure"
    );
    assert!(
        transport
            .query::<Value, _>("mutation Test { test }", json!({}))
            .await
            .is_err()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
