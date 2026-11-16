//! Endpoint timings without URLs, credentials, bodies or personal query values.
use crate::{config::project_key, error::Result};
use http::{Method, StatusCode};
use std::time::Instant;

pub(super) struct RequestLog<'a> {
    method: &'static str,
    endpoint: String,
    project: Option<&'a str>,
    start_at: Option<u64>,
    started: Instant,
    queued: Option<Instant>,
    admitted: Option<Instant>,
    status: Option<u16>,
    outcome: &'static str,
    error_code: Option<&'static str>,
}

impl<'a> RequestLog<'a> {
    pub(super) fn new(method: &Method, path: &str, params: &'a [(&str, String)]) -> Self {
        let project = params
            .iter()
            .find(|(key, _)| matches!(*key, "project" | "projectKey"))
            .map(|(_, value)| value.as_str())
            .filter(|value| safe_project(value));
        let start_at = params
            .iter()
            .find(|(key, _)| *key == "startAt")
            .and_then(|(_, value)| value.parse().ok());
        Self {
            method: match *method {
                Method::GET => "GET",
                Method::POST => "POST",
                _ => "OTHER",
            },
            endpoint: endpoint(path),
            project,
            start_at,
            started: Instant::now(),
            queued: None,
            admitted: None,
            status: None,
            outcome: "cancelled",
            error_code: None,
        }
    }

    pub(super) fn waiting(&mut self) {
        self.queued = Some(Instant::now());
    }

    pub(super) fn admitted(&mut self) {
        self.admitted = Some(Instant::now());
    }

    pub(super) fn response(&mut self, status: StatusCode) {
        self.status = Some(status.as_u16());
    }

    pub(super) fn finish<T>(&mut self, result: &Result<T>) {
        self.outcome = if result.is_ok() { "ok" } else { "error" };
        self.error_code = result.as_ref().err().map(|error| error.code);
    }
}

impl Drop for RequestLog<'_> {
    fn drop(&mut self) {
        let now = Instant::now();
        // Include the full bounded response-body read and JSON parsing, not just
        // response headers. A dropped/cancelled future still produces one event.
        tracing::info!(
            source_id = "jira",
            method = self.method,
            endpoint = %self.endpoint,
            project_key = self.project,
            start_at = self.start_at,
            status = self.status,
            outcome = self.outcome,
            error_code = self.error_code,
            queued_ms = self.queued.map(|at| self.admitted.unwrap_or(now).duration_since(at).as_millis()),
            request_ms = self.admitted.map(|at| now.duration_since(at).as_millis()),
            elapsed_ms = now.duration_since(self.started).as_millis(),
            "Jira request finished"
        );
    }
}

fn safe_project(value: &str) -> bool {
    value.len() <= 100 && project_key(value)
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
}

/// Only known route shapes can enter logs. Keep validated project/type/board IDs
/// useful for diagnostics, but never log an arbitrary path, issue key or query.
fn endpoint(path: &str) -> String {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        [
            "rest",
            "api",
            "3",
            name @ ("myself" | "field" | "mypermissions" | "issue"),
        ] => {
            format!("/rest/api/3/{name}")
        }
        ["rest", "api", "3", "search", "jql"] => "/rest/api/3/search/jql".into(),
        ["rest", "api", "3", "users", "search"] => "/rest/api/3/users/search".into(),
        ["rest", "api", "3", "user", "assignable", "search"] => {
            "/rest/api/3/user/assignable/search".into()
        }
        ["rest", "api", "3", "group", name @ ("bulk" | "member")] => {
            format!("/rest/api/3/group/{name}")
        }
        ["rest", "api", "3", "project", key] if safe_project(key) => {
            format!("/rest/api/3/project/{key}")
        }
        ["rest", "api", "3", "project", key, "statuses"] if safe_project(key) => {
            format!("/rest/api/3/project/{key}/statuses")
        }
        ["rest", "api", "3", "issue", "createmeta", key, "issuetypes"] if safe_project(key) => {
            format!("/rest/api/3/issue/createmeta/{key}/issuetypes")
        }
        [
            "rest",
            "api",
            "3",
            "issue",
            "createmeta",
            key,
            "issuetypes",
            id,
        ] if safe_project(key) && numeric(id) => {
            format!("/rest/api/3/issue/createmeta/{key}/issuetypes/{id}")
        }
        ["rest", "api", "3", "issue", _] => "/rest/api/3/issue/{issue_key}".into(),
        [
            "rest",
            "api",
            "3",
            "issue",
            _,
            name @ ("comment" | "changelog" | "remotelink" | "transitions"),
        ] => {
            format!("/rest/api/3/issue/{{issue_key}}/{name}")
        }
        [
            "rest",
            "agile",
            "1.0",
            "board",
            id,
            name @ ("configuration" | "sprint"),
        ] if numeric(id) => {
            format!("/rest/agile/1.0/board/{id}/{name}")
        }
        ["rest", "agile", "1.0", "sprint", id, "issue"] if numeric(id) => {
            format!("/rest/agile/1.0/sprint/{id}/issue")
        }
        _ => "<unrecognized Jira endpoint>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Upstream, jira::Jira};
    use serde_json::{Value, json};
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };
    use tracing::instrument::WithSubscriber;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl Capture {
        fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
            tracing_subscriber::fmt()
                .with_writer(self.clone())
                .without_time()
                .with_ansi(false)
                .with_target(false)
                .with_max_level(tracing::Level::INFO)
                .finish()
        }

        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    async fn client(server: &MockServer, limits: Upstream) -> Arc<Jira> {
        let config = serde_yaml::from_str(&format!(
            "base_url: {}\nemail: private-email@example.invalid\ntoken: {{value: private-token}}\nprojects: [DEV]\nboard_ids: [12]\nteam: Platform\nsprint_name_patterns: ['.*']\n",
            server.uri()
        )).unwrap();
        Jira::new(&config, &limits).await.unwrap()
    }

    #[test]
    fn known_routes_retain_diagnostic_ids_but_unknown_paths_are_redacted() {
        for path in [
            "rest/api/3/myself",
            "rest/api/3/field",
            "rest/api/3/mypermissions",
            "rest/api/3/search/jql",
            "rest/api/3/group/bulk",
            "rest/api/3/group/member",
            "rest/api/3/users/search",
            "rest/api/3/user/assignable/search",
            "rest/api/3/project/DEV",
            "rest/api/3/project/DEV/statuses",
            "rest/api/3/issue/createmeta/DEV/issuetypes",
            "rest/api/3/issue/createmeta/DEV/issuetypes/10001",
            "rest/agile/1.0/board/12/configuration",
            "rest/agile/1.0/board/12/sprint",
            "rest/agile/1.0/sprint/12/issue",
            "rest/api/3/issue",
        ] {
            assert_eq!(endpoint(path), format!("/{path}"));
        }
        for suffix in ["", "/comment", "/changelog", "/remotelink", "/transitions"] {
            assert_eq!(
                endpoint(&format!("rest/api/3/issue/private-issue{suffix}")),
                format!("/rest/api/3/issue/{{issue_key}}{suffix}")
            );
        }
        for path in [
            "https://private-user:private-token@jira.invalid/rest/api/3/myself",
            "rest/api/3/myself?token=private-token",
            "rest/api/3/group/member#private-fragment",
            "rest/api/3/project/DEV%0Aprivate-token",
            "rest/api/3/issue/createmeta/DEV/issuetypes/private-token",
            "rest/agile/1.0/board/private-token/sprint",
            "private-path",
        ] {
            assert_eq!(endpoint(path), "<unrecognized Jira endpoint>");
        }
    }

    #[tokio::test]
    async fn logs_requests_at_info_with_status_context_timings_and_no_private_values() {
        let server = MockServer::start().await;
        let jira = client(&server, Upstream::default()).await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/field"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "private-etag")
                    .set_body_json(json!(["private-response"])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/field"))
            .and(header("if-none-match", "private-etag"))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        for p in [
            "/rest/api/3/group/bulk",
            "/rest/api/3/group/member",
            "/rest/api/3/user/assignable/search",
            "/rest/api/3/issue/createmeta/DEV/issuetypes/10001",
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/private-issue/remotelink"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-1"})))
            .expect(1)
            .mount(&server)
            .await;

        let capture = Capture::default();
        async {
            for _ in 0..2 {
                let value: Value = jira.get("rest/api/3/field", &[]).await.unwrap();
                assert_eq!(value, json!(["private-response"]));
            }
            let _: Value = jira
                .get(
                    "rest/api/3/group/bulk",
                    &[("groupName", "private-group".into())],
                )
                .await
                .unwrap();
            for start in [0, 50] {
                let _: Value = jira
                    .get(
                        "rest/api/3/group/member",
                        &[
                            ("groupId", "private-group-id".into()),
                            ("startAt", start.to_string()),
                        ],
                    )
                    .await
                    .unwrap();
            }
            let _: Value = jira
                .get(
                    "rest/api/3/user/assignable/search",
                    &[
                        ("project", "DEV".into()),
                        ("accountId", "private-account".into()),
                    ],
                )
                .await
                .unwrap();
            let _: Value = jira
                .get("rest/api/3/issue/createmeta/DEV/issuetypes/10001", &[])
                .await
                .unwrap();
            let _: Value = jira
                .post(
                    "rest/api/3/issue/private-issue/remotelink",
                    &json!({"title":"private-title"}),
                )
                .await
                .unwrap();
            let created = jira
                .create_remote(&super::super::CreateBody {
                    fields: [("summary".into(), json!("private-title"))].into(),
                })
                .await
                .unwrap();
            assert_eq!(created.as_deref(), Some("DEV-1"));
        }
        .with_subscriber(capture.subscriber())
        .await;

        let text = capture.text();
        let events: Vec<_> = text
            .lines()
            .filter(|line| line.contains("Jira request finished"))
            .collect();
        assert_eq!(events.len(), 9, "{text}");
        for event in &events {
            for field in [
                "INFO",
                "source_id=\"jira\"",
                "method=",
                "endpoint=/rest/",
                "status=",
                "outcome=\"ok\"",
                "queued_ms=",
                "request_ms=",
                "elapsed_ms=",
            ] {
                assert!(event.contains(field), "missing {field}: {event}");
            }
        }
        assert!(events.iter().any(|line| line.contains("status=304")));
        assert!(
            events
                .iter()
                .any(|line| line.contains("status=204") && line.contains("method=\"POST\""))
        );
        assert!(events.iter().any(
            |line| line.contains("status=201") && line.contains("endpoint=/rest/api/3/issue ")
        ));
        assert!(
            events
                .iter()
                .any(|line| line.contains("/group/member") && line.contains("start_at=50"))
        );
        assert!(
            events
                .iter()
                .any(|line| line.contains("/user/assignable/search")
                    && line.contains("project_key=\"DEV\""))
        );
        for private in [
            "private-",
            "accountId",
            "groupName",
            "groupId",
            "Authorization",
            "Basic ",
            &server.uri(),
        ] {
            assert!(!text.contains(private), "leaked {private}: {text}");
        }
    }

    #[tokio::test]
    async fn logs_http_json_and_rate_limit_failures_without_raw_errors_or_bodies() {
        let server = MockServer::start().await;
        let jira = client(&server, Upstream::default()).await;
        for (p, status) in [
            ("/rest/api/3/field", 403),
            ("/rest/api/3/project/DEV", 200),
            ("/rest/api/3/group/member", 429),
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(
                    ResponseTemplate::new(status).set_body_string("private-upstream-error"),
                )
                .mount(&server)
                .await;
        }
        let capture = Capture::default();
        async {
            for p in [
                "rest/api/3/field",
                "rest/api/3/project/DEV",
                "rest/api/3/group/member",
                // A subsequent call is suppressed locally, not sent upstream.
                "rest/api/3/group/member",
            ] {
                assert!(jira.get::<Value>(p, &[]).await.is_err());
            }
        }
        .with_subscriber(capture.subscriber())
        .await;
        let text = capture.text();
        let events: Vec<_> = text
            .lines()
            .filter(|line| line.contains("Jira request finished"))
            .collect();
        assert_eq!(events.len(), 4, "{text}");
        for (event, status) in events.iter().zip([403, 200, 429]) {
            assert!(event.contains(&format!("status={status}")), "{event}");
            assert!(event.contains("outcome=\"error\""), "{event}");
        }
        assert!(!events[3].contains("status="), "{}", events[3]);
        assert!(!events[3].contains("request_ms="), "{}", events[3]);
        assert!(events[3].contains("queued_ms="), "{}", events[3]);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
        assert!(text.contains("error_code=\"upstream_backoff\""));
        assert!(!text.contains("private-"));
    }

    #[tokio::test]
    async fn timeouts_log_the_endpoint_without_inventing_a_response_status() {
        let server = MockServer::start().await;
        let jira = client(
            &server,
            Upstream {
                timeout_seconds: 1,
                ..Upstream::default()
            },
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/field"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(2))
                    .set_body_json(json!([])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let capture = Capture::default();
        assert!(
            jira.get::<Value>("rest/api/3/field", &[])
                .with_subscriber(capture.subscriber())
                .await
                .is_err()
        );
        let text = capture.text();
        let events: Vec<_> = text
            .lines()
            .filter(|line| line.contains("Jira request finished"))
            .collect();
        assert_eq!(events.len(), 1, "{text}");
        let event = events[0];
        for field in [
            "endpoint=/rest/api/3/field",
            "outcome=\"error\"",
            "error_code=\"upstream_failure\"",
            "request_ms=",
            "elapsed_ms=",
        ] {
            assert!(event.contains(field), "missing {field}: {event}");
        }
        assert!(!event.contains("status="), "{event}");
        assert!(!text.contains("private-"));
    }

    #[tokio::test]
    async fn budget_waiting_is_logged_without_changing_admission_or_sending_early() {
        let server = MockServer::start().await;
        let jira = client(
            &server,
            Upstream {
                max_concurrent_requests: 1,
                ..Upstream::default()
            },
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/field"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let held = jira
            .budget
            .acquire(crate::request_budget::Bucket::General)
            .await
            .unwrap();
        let capture = Capture::default();
        let mut request = Box::pin(
            jira.get::<Value>("rest/api/3/field", &[])
                .with_subscriber(capture.subscriber()),
        );
        assert!(futures_util::poll!(request.as_mut()).is_pending());
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(!capture.text().contains("Jira request finished"));
        drop(held);
        request.await.unwrap();
        let text = capture.text();
        assert!(text.contains("queued_ms="), "{text}");
        assert!(text.contains("request_ms="), "{text}");
        assert!(text.contains("status=200"), "{text}");
    }

    #[test]
    fn cancellation_is_logged_and_untrusted_context_is_omitted() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.subscriber(), || {
            let params = [
                ("project", "private-project\nINJECTED".into()),
                ("startAt", "private-offset".into()),
            ];
            let mut log = RequestLog::new(&Method::GET, "rest/api/3/group/member", &params);
            log.waiting();
            log.admitted();
            log.response(StatusCode::OK);
            // Dropped before finish, as when the refresh cancels an in-flight body.
        });
        let text = capture.text();
        assert!(text.contains("outcome=\"cancelled\""), "{text}");
        assert!(text.contains("status=200"), "{text}");
        for excluded in ["private-", "INJECTED", "project_key=", "start_at="] {
            assert!(!text.contains(excluded), "{text}");
        }
    }
}
