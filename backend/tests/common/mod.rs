#![allow(dead_code)]
use chrono::{TimeZone, Utc};
use pullboard::{
    config::{Config, ForgeConfig, JiraConfig},
    model::*,
};
use serde_json::json;

pub fn forge(kind: &str, base: &str) -> ForgeConfig {
    serde_yaml::from_str(&format!("id: test-forge\nname: Test Forge\nkind: {kind}\nbase_url: {base}\ntoken: {{value: test-secret}}\n")).unwrap()
}
pub fn jira(base: &str) -> JiraConfig {
    serde_yaml::from_str(&format!("base_url: {base}\nemail: me@example.invalid\ntoken: {{value: test-secret}}\nprojects: [DEV]\nboard_id: 12\nteam: Platform\nsprint_name_pattern: '%YEAR-Platform-S%NUMBER'\n")).unwrap()
}
pub fn config() -> Config {
    let mut c = Config::default();
    c.server.database = ":memory:".into();
    c
}
pub fn person(name: &str) -> Person {
    Person {
        id: name.into(),
        username: name.into(),
        display_name: name.into(),
        url: Some(format!("https://forge.example/{name}")),
    }
}
pub fn pr() -> PullRequest {
    PullRequest {
        id: "test-forge:acme/widget:42".into(),
        source_id: "test-forge".into(),
        forge: Kind::Github,
        number: 42,
        owner: "acme".into(),
        repository: "widget".into(),
        title: "Improve caching DEV-1".into(),
        url: "https://forge.example/acme/widget/pull/42".into(),
        description: "A real upstream description".into(),
        state: "ready".into(),
        author: person("me"),
        assignees: vec![],
        reviewers: vec![],
        labels: vec![],
        created_at: Utc.timestamp_opt(100, 0).unwrap(),
        updated_at: Utc.timestamp_opt(200, 0).unwrap(),
        last_commit_at: None,
        linked_issue_keys: vec!["DEV-1".into()],
        is_mine: true,
        attention: None,
    }
}
pub fn issue() -> JiraIssue {
    JiraIssue {
        id: "100".into(),
        source_id: "jira".into(),
        key: "DEV-1".into(),
        url: "https://jira.example/browse/DEV-1".into(),
        title: "Caching".into(),
        description: String::new(),
        project_key: "DEV".into(),
        issue_type: "Task".into(),
        status: "Open".into(),
        reporter: None,
        assignees: vec![person("me")],
        is_assigned_to_me: true,
        created_at: Utc.timestamp_opt(100, 0).unwrap(),
        updated_at: Utc.timestamp_opt(200, 0).unwrap(),
        sprint_ids: vec![],
        linked_pr_ids: vec![],
        attention: None,
    }
}
pub fn create() -> CreateIssueRequest {
    serde_json::from_value(json!({"pull_request_id":pr().id,"expected_mode_revision":"direct-adapter-test","purpose":"review","project_key":"DEV","issue_type":"Task","title":"Review caching","description":"Please review.","story_points":null,"assignee_id":null,"sprint_id":null,"team":null,"status":null})).unwrap()
}
pub fn github_user(id: u64, name: &str) -> serde_json::Value {
    json!({"id":id,"login":name,"name":name,"html_url":format!("https://github.com/{name}"),"type":"User"})
}
pub fn gitlab_user(id: u64, name: &str) -> serde_json::Value {
    json!({"id":id,"username":name,"name":name,"web_url":format!("https://gitlab.example/{name}")})
}
pub fn forgejo_user(id: u64, name: &str) -> serde_json::Value {
    json!({"id":id,"login":name,"full_name":name,"html_url":format!("https://forgejo.example/{name}")})
}

pub fn identity(source: &str, kind: Kind, account: &str) -> Identity {
    Identity {
        source_id: source.into(),
        kind,
        account_id: account.into(),
        display_name: account.into(),
        username: account.into(),
        profile_url: None,
    }
}
pub async fn verify_sources(cache: &pullboard::cache::Cache, c: &Config) {
    if c.jira.is_some() {
        cache
            .verify(identity("jira", Kind::Jira, "me"))
            .await
            .unwrap();
    }
    for f in &c.forges {
        cache.verify(identity(&f.id, f.kind, "1")).await.unwrap();
    }
}
pub async fn mock_get(server: &wiremock::MockServer, path: &str, body: serde_json::Value) {
    if let Some((issues, filter, value)) = match path {
        "/user" => Some(("/issues", "filter", "assigned")),
        "/api/v4/user" => Some(("/api/v4/issues", "scope", "assigned_to_me")),
        "/api/v1/user" => Some(("/api/v1/repos/issues/search", "type", "issues")),
        _ => None,
    } {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(issues))
            .and(wiremock::matchers::query_param(filter, value))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(json!([]))
                    .insert_header("x-total-count", "0"),
            )
            .with_priority(1)
            .mount(server)
            .await;
    }
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(path))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}
pub fn jira_user(id: &str) -> serde_json::Value {
    json!({"accountId":id,"displayName":id,"emailAddress":format!("{id}@example.invalid"),"active":true,"accountType":"atlassian"})
}
pub fn create_fields() -> Vec<serde_json::Value> {
    vec![
        json!({"fieldId":"project","name":"Project","required":true,"schema":{"type":"project"},"operations":["set"]}),
        json!({"fieldId":"issuetype","name":"Issue type","required":true,"schema":{"type":"issuetype"},"operations":["set"]}),
        json!({"fieldId":"summary","name":"Summary","required":true,"schema":{"type":"string"},"operations":["set"]}),
        json!({"fieldId":"description","name":"Description","required":false,"schema":{"type":"string"},"operations":["set"]}),
        json!({"fieldId":"assignee","name":"Assignee","required":false,"schema":{"type":"user"},"operations":["set"]}),
        json!({"fieldId":"customfield_712","name":"AssignedTeam","required":false,"schema":{"type":"option","custom":"com.atlassian.jira.plugin.system.customfieldtypes:select"},"operations":["set"],"allowedValues":[{"id":"889","value":"Platform"},{"id":"890","value":"Independent"}]}),
        json!({"fieldId":"customfield_913","name":"Points","required":false,"schema":{"type":"number"},"operations":["set"]}),
    ]
}
pub async fn mock_jira(server: &wiremock::MockServer) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/rest/api/3/search/jql"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(json!({"issues":[],"isLast":true})),
        )
        .with_priority(6)
        .mount(server)
        .await;
    mock_get(server, "/rest/api/3/myself", jira_user("me")).await;
    mock_get(server, "/rest/api/3/field", json!([
        {"id":"customfield_711","key":"customfield_711","name":"Sprint","schema":{"type":"array","items":"json","custom":"com.pyxis.greenhopper.jira:gh-sprint"}},
        {"id":"customfield_712","name":"AssignedTeam","schema":{"type":"option","custom":"com.atlassian.jira.plugin.system.customfieldtypes:select"}},
        {"id":"customfield_913","name":"Points","schema":{"type":"number"}}
    ])).await;
    mock_get(server, "/rest/agile/1.0/board/12/configuration", json!({"id":12,"type":"scrum","estimation":{"type":"field","field":{"fieldId":"customfield_913"}}})).await;
    mock_get(
        server,
        "/rest/api/3/project/DEV",
        json!({"key":"DEV","name":"Development"}),
    )
    .await;
    mock_get(server, "/rest/api/3/project/DEV/statuses", json!([
        {"id":"1","name":"Task","statuses":[{"id":"10","name":"Open"},{"id":"20","name":"Review"},{"id":"30","name":"QA validation"}]},
        {"id":"2","name":"Story","statuses":[{"id":"10","name":"Open"},{"id":"40","name":"In progress"}]}
    ])).await;
    mock_get(server, "/rest/api/3/mypermissions", json!({"permissions":{
        "CREATE_ISSUES":{"havePermission":true},"EDIT_ISSUES":{"havePermission":true},"SCHEDULE_ISSUES":{"havePermission":true},"ASSIGN_ISSUES":{"havePermission":true}
    }})).await;
    mock_get(server, "/rest/api/3/issue/createmeta/DEV/issuetypes", json!({"startAt":0,"maxResults":100,"total":2,"issueTypes":[{"id":"1","name":"Task"},{"id":"2","name":"Story"}]})).await;
    for id in [1, 2] {
        let fields = create_fields();
        mock_get(
            server,
            &format!("/rest/api/3/issue/createmeta/DEV/issuetypes/{id}"),
            json!({"startAt":0,"maxResults":100,"total":fields.len(),"fields":fields}),
        )
        .await;
    }
    mock_get(
        server,
        "/rest/agile/1.0/board/12/sprint",
        json!({"startAt":0,"maxResults":100,"isLast":true,"values":[]}),
    )
    .await;
    mock_all_users(server, vec![jira_user("me"), jira_user("other")]).await;
    mock_assignable(server, vec![jira_user("me"), jira_user("other")]).await;
}
pub async fn bind_jira(jira: &pullboard::jira::Jira) -> std::sync::Arc<pullboard::cache::Cache> {
    let mut config = config();
    config.jira = Some(jira.config.clone());
    let cache = std::sync::Arc::new(
        pullboard::cache::Cache::open(&config.server.database, &config)
            .await
            .unwrap(),
    );
    jira.bind_cache(cache.clone());
    cache
}
pub async fn ready_jira(jira: &pullboard::jira::Jira) {
    bind_jira(jira).await;
    jira.refresh_metadata().await.unwrap();
}
pub async fn mock_all_users(server: &wiremock::MockServer, users: Vec<serde_json::Value>) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/rest/api/3/users/search"))
        .respond_with(move |request: &wiremock::Request| {
            let param = |key: &str| {
                request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| v.parse::<usize>().ok())
            };
            let values: Vec<_> = users
                .iter()
                .skip(param("startAt").unwrap_or(0))
                .take(param("maxResults").unwrap_or(100))
                .cloned()
                .collect();
            wiremock::ResponseTemplate::new(200).set_body_json(values)
        })
        .with_priority(6)
        .mount(server)
        .await;
}
pub async fn mock_assignable(server: &wiremock::MockServer, users: Vec<serde_json::Value>) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/rest/api/3/user/assignable/search",
        ))
        .respond_with(move |request: &wiremock::Request| {
            let Some((_, id)) = request.url.query_pairs().find(|(k, _)| k == "accountId") else {
                return wiremock::ResponseTemplate::new(500);
            };
            let values: Vec<_> = users
                .iter()
                .filter(|u| u["accountId"].as_str() == Some(id.as_ref()))
                .cloned()
                .collect();
            wiremock::ResponseTemplate::new(200).set_body_json(values)
        })
        .with_priority(6)
        .mount(server)
        .await;
}

pub fn validate_contract(value: &serde_json::Value, name: &str) {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../api/openapi.json")).unwrap();
    let validator = jsonschema::validator_for(&json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "$ref":format!("#/components/schemas/{name}"),"components":doc["components"]
    }))
    .unwrap();
    if let Err(error) = validator.validate(value) {
        panic!("{name}: {error}; {value}");
    }
}
