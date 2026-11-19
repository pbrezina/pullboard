use http::HeaderMap;
use pullboard_core::{
    config::{Config, SprintPattern, Upstream},
    transport::{ReadHttp, ReadTransport},
};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[test]
fn validated_yaml_roundtrip_preserves_existing_settings_and_strict_optional_policies() {
    let input = r"
server:
  bind: 0.0.0.0
  port: 8181
  allowed_origins: [http://localhost:3333, http://127.0.0.1:3333]
  static_directory: /app/frontend
  database: /data/cache.sqlite
read_only_test_mode: false
refresh_interval_seconds: 1234
upstream: {page_size: 17, max_pages: 23, timeout_seconds: 19, max_concurrent_requests: 2}
jira:
  base_url: https://jira.example
  email: developer@example.invalid
  token: {keyring: {service: saved-service, account: saved-account}}
  projects: [PRIMARY, SECONDARY]
  board_ids: [42]
  team: Platform
  team_field_name: AssignedTeam
  sprint_name_patterns: ['(?i)platform']
  assignee_groups: [engineers, reviewers]
  defaults:
    implementation: {issue_type: Story, assignee: me, workflow: {draft: Coding, ready: null}}
  project_overrides:
    SECONDARY:
      defaults:
        review: {issue_type: Task, assignee: null}
forges:
  - id: unchanged-stable-id
    name: Work forge
    kind: gitlab
    base_url: https://forge.example/subpath
    token: {value: literal-test-secret}
    include_repositories: [acme/*]
    exclude_repositories: [acme/archived]
    default_project: SECONDARY
ai:
  enabled: true
  provider: anthropic
  model: exact-user-model
  base_url: https://ai.example/api
  token: {value: ai-test-secret}
";
    let config = Config::from_yaml(input).unwrap();
    let yaml = config.to_yaml().unwrap();
    let restored = Config::from_yaml(&yaml).unwrap();
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        serde_json::to_value(&restored).unwrap()
    );
    assert_eq!(
        restored.jira.as_ref().unwrap().projects,
        ["PRIMARY", "SECONDARY"]
    );
    assert_eq!(restored.forges[0].id, "unchanged-stable-id");
    assert!(!restored.read_only_test_mode);
    assert!(!format!("{restored:?}").contains("literal-test-secret"));
    assert!(
        restored
            .jira
            .as_ref()
            .unwrap()
            .allows_sprint("2026-PLATFORM-S1")
    );
    // Absent purpose policies must not serialize as rejected explicit null objects.
    Config::from_yaml(&Config::default().to_yaml().unwrap()).unwrap();
}

#[test]
fn config_errors_never_include_raw_yaml_or_secrets() {
    for yaml in [
        "unknown: private-token",
        "ai: {token: {value: private-token, env: PRIVATE}}",
        "ai: [private-token",
    ] {
        let error = Config::from_yaml(yaml).unwrap_err();
        assert!(!format!("{error:?}").contains("private-token"));
    }
    assert!(Config::from_yaml(&" ".repeat(Config::MAX_YAML_BYTES + 1)).is_err());
    let mut config = Config::default();
    config.server.port = 1;
    assert!(config.to_yaml().is_err());
}

#[test]
fn regex_serialization_is_a_string_with_preserved_search_semantics() {
    let pattern = SprintPattern::new("(?i)identity").unwrap();
    let yaml = serde_yaml::to_string(&pattern).unwrap();
    let restored: SprintPattern = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(restored.as_str(), "(?i)identity");
    assert!(restored.is_match("2026-IDENTITY-S1"));
    assert!(SprintPattern::new("(?:a{1000}){1000}").is_err());
    let config = Config::default();
    config.validate().unwrap();
    config.validate().unwrap();
}

#[cfg(not(feature = "native-keyring"))]
#[tokio::test]
async fn default_feature_set_preserves_but_never_resolves_keyring_references() {
    use pullboard_core::config::Secret;
    let secret: Secret =
        serde_yaml::from_str("keyring: {service: private-service, account: private-account}")
            .unwrap();
    let error = secret.resolve().await.unwrap_err().to_string();
    assert!(error.contains("not enabled"));
    assert!(!error.contains("private-service"));
    assert!(!error.contains("private-account"));
    assert!(serde_yaml::to_string(&secret).unwrap().contains("keyring:"));
}

#[tokio::test]
async fn configuration_reads_are_get_only_scoped_and_header_authenticated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tenant/rest/api/3/myself"))
        .and(header("authorization", "Basic ZGV2ZWxvcGVyOnNlY3JldA=="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"accountId":"real-mock-id"})))
        .expect(1)
        .mount(&server)
        .await;
    let config = Config::from_yaml(&format!("jira: {{base_url: {}/tenant, email: developer, token: {{value: secret}}, projects: [DEV], board_ids: [1], team: Platform, sprint_name_patterns: [Platform]}}", server.uri())).unwrap();
    let http = ReadHttp::jira(config.jira.as_ref().unwrap(), &config.upstream)
        .await
        .unwrap();
    let value: Value = http.get("rest/api/3/myself", &[]).await.unwrap();
    assert_eq!(value["accountId"], "real-mock-id");
    for invalid in [
        "https://evil.example/user",
        "../user",
        "rest/user?token=secret",
        "rest/user#fragment",
    ] {
        assert!(http.get::<Value>(invalid, &[]).await.is_err());
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn shared_helper_rejects_redirects_large_bodies_and_implicit_retries() {
    let server = MockServer::start().await;
    for (route, response) in [
        (
            "redirect",
            ResponseTemplate::new(302).insert_header("location", "/destination"),
        ),
        (
            "large",
            ResponseTemplate::new(200).set_body_string("x".repeat(1025)),
        ),
        (
            "failure",
            ResponseTemplate::new(500).set_body_string("private-upstream-error"),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{route}")))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
    }
    let http = ReadHttp::new(
        &server.uri(),
        HeaderMap::new(),
        &Upstream {
            max_response_bytes: 1024,
            ..Default::default()
        },
    )
    .unwrap();
    for route in ["redirect", "large", "failure"] {
        let error = http.get::<Value>(route, &[]).await.unwrap_err();
        assert!(!format!("{error:?}").contains("private-upstream-error"));
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn jira_helper_and_forge_transport_share_origin_backoff() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/myself"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .expect(1)
        .mount(&server)
        .await;
    let http = ReadHttp::new(&server.uri(), HeaderMap::new(), &Upstream::default()).unwrap();
    let forge = serde_yaml::from_str(&format!(
        "id: mock\nkind: forgejo\nbase_url: {}\ntoken: {{value: mock-secret}}",
        server.uri()
    ))
    .unwrap();
    let forge = ReadTransport::new(&forge, &Upstream::default())
        .await
        .unwrap();
    assert_eq!(
        http.get::<Value>("rest/api/3/myself", &[])
            .await
            .unwrap_err()
            .code,
        "upstream_backoff"
    );
    assert_eq!(
        forge.get::<Value>("user", &[]).await.unwrap_err().code,
        "upstream_backoff"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn helper_rejects_unbounded_transport_configuration() {
    let limits = Upstream {
        timeout_seconds: 0,
        ..Default::default()
    };
    assert!(ReadHttp::new("https://jira.example", HeaderMap::new(), &limits).is_err());
}
#[test]
fn optional_attention_window_default_and_bounds() {
    assert_eq!(Config::from_yaml("{}").unwrap().attention_default_days, 14);
    assert_eq!(
        Config::from_yaml("attention_default_days: 45")
            .unwrap()
            .attention_default_days,
        45
    );
    assert!(Config::from_yaml("attention_default_days: 0").is_err());
    assert!(Config::from_yaml("attention_default_days: 36501").is_err());
}
