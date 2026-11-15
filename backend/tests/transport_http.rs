//! Wire compatibility and safety at the shared read-only forge boundary.
mod common;

use axum::{Router, body::Body, extract::Request, response::Response};
use common::forge;
use futures_util::{StreamExt, stream};
use pullboard::{config::Upstream, transport::ReadTransport};
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

const FORGES: [(&str, &str); 4] = [
    ("github", ""),
    ("gitlab", "/api/v4"),
    ("codeberg", "/api/v1"),
    ("forgejo", "/api/v1"),
];

#[tokio::test]
async fn construction_is_offline_and_each_forge_sends_its_expected_auth_and_headers() {
    for (kind, prefix) in FORGES {
        let server = MockServer::start().await;
        let transport = ReadTransport::new(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
        Mock::given(method("GET"))
            .and(path(format!("{prefix}/user")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
            .expect(1)
            .mount(&server)
            .await;
        transport.get::<Value>("user", &[]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let headers = &requests[0].headers;
        match kind {
            "github" => {
                assert_eq!(headers["authorization"], "Bearer test-secret");
                assert_eq!(headers["user-agent"], "octocrab");
                assert_eq!(headers["x-github-api-version"], "2022-11-28");
                assert_eq!(headers["content-length"], "0");
                assert!(!headers.contains_key("private-token"));
            }
            "gitlab" => {
                assert_eq!(headers["private-token"], "test-secret");
                assert_eq!(headers["user-agent"], "PullBoard/0.1");
                assert!(!headers.contains_key("authorization"));
                assert!(!headers.contains_key("x-github-api-version"));
            }
            _ => {
                assert_eq!(headers["authorization"], "token test-secret");
                assert_eq!(headers["user-agent"], "PullBoard/0.1");
                assert!(!headers.contains_key("private-token"));
                assert!(!headers.contains_key("x-github-api-version"));
            }
        }
        assert!(!headers.contains_key("content-type"));
    }
}

#[tokio::test]
async fn invalid_auth_headers_fail_without_network_or_secret_disclosure() {
    let server = MockServer::start().await;
    for (kind, _) in FORGES {
        let mut config = forge(kind, &server.uri());
        config.token = serde_yaml::from_str(r#"value: "private\ninvalid-token""#).unwrap();
        let error = ReadTransport::new(&config, &Upstream::default())
            .await
            .err()
            .expect("invalid header must not panic or initialize a client");
        assert!(!error.to_string().contains("private"));
        assert!(!error.to_string().contains("invalid-token"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn same_origin_instances_keep_credentials_and_etag_caches_separate() {
    for (kind, prefix) in FORGES {
        let server = MockServer::start().await;
        let mut transports = Vec::new();
        for identity in ["alpha", "beta"] {
            let mut config = forge(kind, &server.uri());
            config.id = identity.into();
            config.token = serde_yaml::from_str(&format!("value: {identity}")).unwrap();
            let (name, value) = match kind {
                "github" => ("authorization", format!("Bearer {identity}")),
                "gitlab" => ("private-token", identity.to_owned()),
                _ => ("authorization", format!("token {identity}")),
            };
            Mock::given(method("GET"))
                .and(path(format!("{prefix}/private")))
                .and(header(name, value.as_str()))
                .and(|request: &wiremock::Request| !request.headers.contains_key("if-none-match"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("etag", format!("\"{identity}\""))
                        .set_body_json(json!({"identity": identity})),
                )
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("{prefix}/private")))
                .and(header(name, value.as_str()))
                .and(header("if-none-match", format!("\"{identity}\"")))
                .respond_with(ResponseTemplate::new(304))
                .expect(1)
                .mount(&server)
                .await;
            transports.push(
                ReadTransport::new(&config, &Upstream::default())
                    .await
                    .unwrap(),
            );
        }
        for _ in 0..2 {
            for (transport, identity) in transports.iter().zip(["alpha", "beta"]) {
                assert_eq!(
                    transport.get::<Value>("private", &[]).await.unwrap(),
                    json!({"identity": identity})
                );
            }
        }
    }
}

#[tokio::test]
async fn configured_subpaths_and_github_enterprise_rest_and_graphql_paths_are_preserved() {
    for (kind, base, rest, graphql) in [
        (
            "github",
            "/proxy/api/v3",
            "/proxy/api/v3/user",
            Some("/proxy/api/graphql"),
        ),
        ("github", "/api/v3/", "/api/v3/user", Some("/api/graphql")),
        ("github", "/proxy/", "/proxy/user", Some("/proxy/graphql")),
        ("gitlab", "/proxy", "/proxy/api/v4/user", None),
        ("codeberg", "/proxy/", "/proxy/api/v1/user", None),
        ("forgejo", "/proxy", "/proxy/api/v1/user", None),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(rest))
            .and(query_param("search", "nested/group + &"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let transport = ReadTransport::new(
            &forge(kind, &format!("{}{base}", server.uri())),
            &Upstream::default(),
        )
        .await
        .unwrap();
        transport
            .get::<Value>("/user", &[("search", "nested/group + &".into())])
            .await
            .unwrap();
        if let Some(graphql) = graphql {
            let query = "query Test($text: String!) { viewer { login } }";
            let variables = json!({"text": "mutation is untrusted data, not an operation"});
            Mock::given(method("POST"))
                .and(path(graphql))
                .and(header("authorization", "Bearer test-secret"))
                .and(header("user-agent", "octocrab"))
                .and(header("x-github-api-version", "2022-11-28"))
                .and(header("content-type", "application/json"))
                .and(body_json(json!({"query": query, "variables": variables})))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {}})))
                .expect(1)
                .mount(&server)
                .await;
            transport.query::<Value, _>(query, variables).await.unwrap();
        }
    }
}

#[tokio::test]
async fn off_origin_gets_and_same_origin_redirects_never_make_followup_requests() {
    for (kind, prefix) in FORGES {
        let server = MockServer::start().await;
        let target = MockServer::start().await;
        let transport = ReadTransport::new(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        assert!(
            transport
                .get::<Value>(&format!("{}/private", target.uri()), &[])
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(target.received_requests().await.unwrap().is_empty());
        Mock::given(method("GET"))
            .and(path(format!("{prefix}/redirect")))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/private"))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            transport
                .get::<Value>("redirect", &[])
                .await
                .unwrap_err()
                .code,
            "upstream_failure"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn graphql_errors_and_redirects_are_not_retried_or_followed() {
    let server = MockServer::start().await;
    let target = MockServer::start().await;
    let transport = ReadTransport::new(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    for response in [
        ResponseTemplate::new(503).set_body_string("private upstream failure"),
        ResponseTemplate::new(302).insert_header("location", format!("{}/private", target.uri())),
        ResponseTemplate::new(200).set_body_string("private invalid JSON"),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = transport
            .query::<Value, _>("query Test { viewer { login } }", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "upstream_failure");
        assert!(!error.message.contains("private"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(target.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn http_and_json_errors_are_sanitized_and_content_length_bounds_are_enforced() {
    for (kind, _) in FORGES {
        let server = MockServer::start().await;
        let limits = Upstream {
            max_response_bytes: 16,
            ..Default::default()
        };
        let transport = ReadTransport::new(&forge(kind, &server.uri()), &limits)
            .await
            .unwrap();
        for (status, body, code) in [
            (404, "private", "upstream_not_found"),
            (401, "private", "upstream_failure"),
            (403, "private", "upstream_failure"),
            (200, "private", "upstream_failure"),
            (200, "\"private oversized response\"", "upstream_failure"),
            (304, "", "upstream_failure"),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
            let error = transport.get::<Value>("test", &[]).await.unwrap_err();
            assert_eq!(error.status, http::StatusCode::BAD_GATEWAY);
            assert_eq!(error.code, code, "{kind}: {status}");
            assert!(!error.message.contains("private"));
            assert!(!error.message.contains("test-secret"));
        }
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("\"12345678901234\""))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            transport.get::<String>("test", &[]).await.unwrap(),
            "12345678901234"
        );
    }
}

struct StreamingServer {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for StreamingServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn streaming_server() -> StreamingServer {
    async fn handler(request: Request) -> Response {
        let scenario = request.uri().path().split('/').nth(1).unwrap().to_owned();
        if scenario == "slow-headers" {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let body = if scenario == "broken" {
            Body::from_stream(stream::once(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Err::<&'static str, _>(std::io::Error::other("private upstream body failure"))
            }))
        } else {
            Body::from_stream(
                stream::iter(["\"1234567", "89012345\""]).then(move |chunk| {
                    let slow = scenario == "slow-body";
                    async move {
                        if slow {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                        Ok::<_, Infallible>(chunk)
                    }
                }),
            )
        };
        Response::new(body)
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(handler))
            .await
            .unwrap();
    });
    StreamingServer { base, task }
}

#[tokio::test]
async fn streamed_bounds_failures_and_header_and_body_timeouts_cover_rest_and_graphql() {
    let server = streaming_server().await;
    let limits = Upstream {
        max_response_bytes: 16,
        timeout_seconds: 1,
        ..Default::default()
    };
    for scenario in ["chunked", "broken", "slow-headers", "slow-body"] {
        for (kind, _) in FORGES {
            let transport = ReadTransport::new(
                &forge(kind, &format!("{}/{scenario}", server.base)),
                &limits,
            )
            .await
            .unwrap();
            let error =
                tokio::time::timeout(Duration::from_secs(3), transport.get::<Value>("test", &[]))
                    .await
                    .expect("request must not outlive its configured timeout")
                    .unwrap_err();
            assert_eq!(error.code, "upstream_failure", "{kind}: {scenario}");
            assert!(!error.message.contains("private"));
            if kind == "github" {
                let error = tokio::time::timeout(
                    Duration::from_secs(3),
                    transport.query::<Value, _>("query Test { viewer { login } }", json!({})),
                )
                .await
                .expect("GraphQL must not outlive its configured timeout")
                .unwrap_err();
                assert_eq!(error.code, "upstream_failure", "GraphQL: {scenario}");
            }
        }
    }
}

#[tokio::test]
async fn only_explicit_github_query_documents_can_reach_graphql() {
    let server = MockServer::start().await;
    for (kind, _) in FORGES {
        let transport = ReadTransport::new(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        for query in [
            "mutation Write { addComment { id } }",
            "query Read { viewer { login } } mutation Write { addComment { id } }",
            "subscription Watch { viewer { login } }",
            "{ viewer { login } }",
            "",
        ] {
            assert!(transport.query::<Value, _>(query, json!({})).await.is_err());
        }
        if kind != "github" {
            assert!(
                transport
                    .query::<Value, _>("query Read { viewer { login } }", json!({}))
                    .await
                    .is_err()
            );
        }
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
