mod common;
use common::*;
use pullboard::{config::Upstream, jira::Jira, model::Purpose};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

async fn override_get(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(1)
        .mount(server)
        .await;
}
async fn field_page(server: &MockServer, type_id: u64, fields: Vec<Value>) {
    override_get(
        server,
        &format!("/rest/api/3/issue/createmeta/DEV/issuetypes/{type_id}"),
        json!({"startAt":0,"maxResults":100,"total":fields.len(),"fields":fields}),
    )
    .await;
}
async fn sprint_page(server: &MockServer, values: Value) {
    override_get(
        server,
        "/rest/agile/1.0/board/12/sprint",
        json!({"startAt":0,"maxResults":100,"isLast":true,"values":values}),
    )
    .await;
}
async fn ready(server: &MockServer) -> std::sync::Arc<Jira> {
    let j = Jira::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    bind_jira(&j).await;
    let _ = j.refresh_metadata().await;
    j
}
fn sprint_field() -> Value {
    json!({"fieldId":"customfield_711","name":"Sprint","required":false,"schema":{"type":"array","items":"json","custom":"com.pyxis.greenhopper.jira:gh-sprint"},"operations":["set"]})
}
fn contextual_issue_type_field() -> Value {
    json!({"fieldId":"issuetype","name":"Issue Type","required":true,"hasDefaultValue":false,"schema":{"type":"issuetype","system":"issuetype"},"operations":[]})
}

#[tokio::test]
async fn contextual_issue_type_with_empty_operations_supports_cached_options_and_simulation() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    for id in [1, 2] {
        let mut fields = create_fields();
        fields[1] = contextual_issue_type_field();
        fields.push(json!({"fieldId":"reporter","name":"Reporter","required":true,"hasDefaultValue":true,"schema":{"type":"user","system":"reporter"},"operations":["set"]}));
        field_page(&server, id, fields).await;
    }
    let j = ready(&server).await;
    let before = server.received_requests().await.unwrap().len();
    for issue_type in ["Task", "Story"] {
        for purpose in [Purpose::Implementation, Purpose::Review] {
            let options = j
                .options("DEV", purpose, Some(issue_type), "")
                .await
                .unwrap();
            assert!(options.capabilities.can_create);
            assert!(options.capabilities.blockers.is_empty());
            validate_contract(&serde_json::to_value(&options).unwrap(), "JiraOptions");
            let mut request = create();
            request.purpose = purpose;
            request.issue_type = issue_type.into();
            let result = j.create(&request, &pr(), &[], true).await.unwrap();
            assert_eq!(result.outcome, "simulated");
            assert!(result.issue.is_none());
            assert!(result.warnings.is_empty());
        }
    }
    let mut request = create();
    request.issue_type = "Unknown".into();
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );
    let log = server.received_requests().await.unwrap();
    assert_eq!(log.len(), before);
    assert!(log.iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn contextual_issue_type_with_empty_operations_sends_selected_discovered_id_on_mock_create() {
    for (issue_type, id) in [("Task", 1), ("Story", 2)] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let mut fields = create_fields();
        fields[1] = contextual_issue_type_field();
        field_page(&server, id, fields).await;
        let j = ready(&server).await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .and(body_partial_json(json!({"fields":{
                "project":{"key":"DEV"},"issuetype":{"id":id.to_string()}
            }})))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-50"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/DEV-50/remotelink"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let mut request = create();
        request.issue_type = issue_type.into();
        let result = j.create(&request, &pr(), &[], false).await.unwrap();
        assert_eq!(result.outcome, "created");
        assert_eq!(result.issue.unwrap().key, "DEV-50");
        assert!(result.warnings.is_empty());
        assert_eq!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.method != "GET")
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn contextual_issue_type_exception_does_not_bypass_other_required_field_checks() {
    for scenario in [
        "missing_issue_type",
        "missing_issue_type_schema",
        "wrong_issue_type_schema",
        "wrong_issue_type_system",
        "unsupported_issue_type_operation",
        "project_without_set",
        "summary_without_set",
        "required_custom_issue_type_schema",
        "required_custom_with_set",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let mut fields = create_fields();
        fields[1] = contextual_issue_type_field();
        match scenario {
            "missing_issue_type" => fields.retain(|f| f["fieldId"] != "issuetype"),
            "missing_issue_type_schema" => fields[1]["schema"] = json!({}),
            "wrong_issue_type_schema" => fields[1]["schema"]["type"] = json!("string"),
            "wrong_issue_type_system" => fields[1]["schema"]["system"] = json!("project"),
            "unsupported_issue_type_operation" => fields[1]["operations"] = json!(["add"]),
            "project_without_set" => fields[0]["operations"] = json!([]),
            "summary_without_set" => fields[2]["operations"] = json!([]),
            "required_custom_issue_type_schema" => {
                let mut field = contextual_issue_type_field();
                field["fieldId"] = json!("customfield_42");
                fields.push(field);
            }
            "required_custom_with_set" => fields.push(json!({"fieldId":"customfield_42","name":"Required custom field","required":true,"hasDefaultValue":false,"schema":{"type":"string"},"operations":["set"]})),
            _ => unreachable!(),
        }
        field_page(&server, 1, fields).await;
        let j = ready(&server).await;
        let options = j.options("DEV", Purpose::Review, None, "").await.unwrap();
        assert!(!options.capabilities.can_create, "{scenario}");
        assert!(
            options
                .capabilities
                .blockers
                .iter()
                .any(|d| d.code == "required_field_unsupported"),
            "{scenario}"
        );
        for read_only in [true, false] {
            assert_eq!(
                j.create(&create(), &pr(), &[], read_only)
                    .await
                    .unwrap_err()
                    .code,
                "create_blocked",
                "{scenario}"
            );
        }
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method == "GET"),
            "{scenario}"
        );
    }
}

#[tokio::test]
async fn type_specific_capabilities_defaults_and_search_do_not_change_authorization() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut story = create_fields();
    story.retain(|f| f["fieldId"] != "customfield_913");
    field_page(&server, 2, story).await;
    sprint_page(
        &server,
        json!([
            {"id":501,"name":"2026-Platform-S1","state":"active","originBoardId":1000},
            {"id":502,"name":"CORE","state":"active"},
            {"id":503,"name":"2026-Platform-S2","state":"future"}
        ]),
    )
    .await;
    let j = ready(&server).await;
    let implementation = j
        .options("DEV", Purpose::Implementation, None, "not-found")
        .await
        .unwrap();
    assert!(implementation.assignees.is_empty());
    assert!(implementation.defaults.assignee_id.is_none());
    assert!(implementation.defaults.sprint_id.is_none());
    assert_eq!(implementation.capabilities.sprint.mode, "follow_up");
    assert_eq!(implementation.sprints.len(), 3);
    assert!(
        implementation
            .sprints
            .iter()
            .all(|s| s.team.is_none() && s.board_id == 12)
    );
    let review = j
        .options("DEV", Purpose::Review, Some("Story"), "")
        .await
        .unwrap();
    assert_eq!(review.defaults.issue_type.as_deref(), Some("Task"));
    assert!(review.defaults.assignee_id.is_none());
    assert!(review.assignees.iter().all(|p| p.id != "me"));
    assert_eq!(review.capabilities.story_points.mode, "unsupported");
    assert_eq!(implementation.capabilities.story_points.mode, "create");
    assert_eq!(j.projects().await[0].name, "Development");
    assert_eq!(
        j.options("DEV", Purpose::Review, Some("Epic"), "")
            .await
            .unwrap_err()
            .code,
        "invalid_choice"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().contains("/board/"))
            .all(|r| r.url.path().starts_with("/rest/agile/1.0/board/12/"))
    );
}

#[tokio::test]
async fn zero_one_many_default_sprints_never_choose_first_and_future_remains_available() {
    for count in [0, 1, 2] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let mut values = vec![json!({"id":99,"name":"2026-Platform-S0","state":"future"})];
        for id in 1..=count {
            values.push(json!({"id":id,"name":format!("1999-Platform-S{id}"),"state":"active"}));
        }
        sprint_page(&server, json!(values)).await;
        let j = ready(&server).await;
        let o = j
            .options("DEV", Purpose::Implementation, None, "")
            .await
            .unwrap();
        assert_eq!(o.sprints.len(), count + 1);
        assert!(o.defaults.sprint_id.is_none());
        assert!(
            !o.diagnostics
                .iter()
                .any(|d| d.code.starts_with("sprint_default_"))
        );
    }
}

#[tokio::test]
async fn absent_default_type_is_not_replaced_by_first_type() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    override_get(
        &server,
        "/rest/api/3/issue/createmeta/DEV/issuetypes",
        json!({"startAt":0,"total":1,"issueTypes":[{"id":"2","name":"Story"}]}),
    )
    .await;
    let j = ready(&server).await;
    let o = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert!(o.issue_type.is_none());
    assert!(o.defaults.issue_type.is_none());
    assert_eq!(o.capabilities.sprint.mode, "unknown");
    assert!(!o.capabilities.can_create);
    assert!(o.teams.is_empty());
    assert_eq!(o.capabilities.blockers[0].code, "issue_type_required");
    assert!(
        o.diagnostics
            .iter()
            .any(|d| d.code == "issue_type_default_unavailable")
    );
}

#[tokio::test]
async fn create_uses_discovered_select_object_numeric_points_and_sprint_not_labels_or_ids_from_yaml()
 {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut fields = create_fields();
    fields.push(sprint_field());
    field_page(&server, 1, fields).await;
    sprint_page(
        &server,
        json!([{"id":875,"name":"Foreign","state":"future","originBoardId":99}]),
    )
    .await;
    let j = ready(&server).await;
    let o = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert_eq!(o.capabilities.sprint.mode, "create");
    let independent = o.teams.iter().find(|t| t.name == "Independent").unwrap();
    let mut request = create();
    request.team = Some(independent.id.clone());
    request.sprint_id = Some("875".into());
    request.story_points = Some(3.5);
    Mock::given(method("POST")).and(path("/rest/api/3/issue"))
        .and(body_partial_json(json!({"fields":{"issuetype":{"id":"1"},"customfield_712":{"id":"890"},"customfield_913":3.5,"customfield_711":875}})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-50"}))).expect(1).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/DEV-50/remotelink"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let r = j.create(&request, &pr(), &[], false).await.unwrap();
    assert_eq!(r.issue.unwrap().key, "DEV-50");
    assert!(r.warnings.is_empty());
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().ends_with("/875/issue"))
    );
}

#[tokio::test]
async fn sprint_absent_create_screen_uses_documented_followup_and_failure_is_warning_not_duplicate_error()
 {
    for status in [204, 403] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        sprint_page(
            &server,
            json!([{"id":1234,"name":"2026-Platform-S1","state":"active"}]),
        )
        .await;
        let j = ready(&server).await;
        let mut request = create();
        request.sprint_id = Some("1234".into());
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"key":"DEV-51"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/agile/1.0/sprint/1234/issue"))
            .and(body_json(json!({"issues":["DEV-51"]})))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/DEV-51/remotelink"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let r = j.create(&request, &pr(), &[], false).await.unwrap();
        assert_eq!(r.issue.unwrap().key, "DEV-51");
        assert_eq!(r.warnings.len(), usize::from(status != 204));
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests
            .iter()
            .find(|r| r.method == "POST" && r.url.path() == "/rest/api/3/issue")
            .unwrap()
            .body_json()
            .unwrap();
        assert!(body["fields"].get("customfield_711").is_none());
        let writes: Vec<_> = requests.iter().filter(|r| r.method != "GET").collect();
        assert_eq!(writes[0].url.path(), "/rest/api/3/issue");
        assert_eq!(writes[1].url.path(), "/rest/agile/1.0/sprint/1234/issue");
    }
}

#[tokio::test]
async fn simulation_validates_team_points_sprint_status_but_request_log_has_zero_write_verbs() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    sprint_page(
        &server,
        json!([{"id":1234,"name":"2026-Platform-S1","state":"active"}]),
    )
    .await;
    let j = ready(&server).await;
    let o = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    let mut r = create();
    r.purpose = Purpose::Implementation;
    r.team = o.defaults.team;
    r.assignee_id = o.defaults.assignee_id;
    r.sprint_id = Some("1234".into());
    r.story_points = Some(5.0);
    r.status = Some("Review".into());
    let result = j.create(&r, &pr(), &[], true).await.unwrap();
    assert_eq!(result.outcome, "simulated");
    assert!(result.issue.is_none());
    j.link("DEV-1", &pr(), false, true).await.unwrap();
    let log = server.received_requests().await.unwrap();
    assert!(!log.is_empty());
    assert!(log.iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn followup_requires_discovered_scrum_sprint_and_project_permissions() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    override_get(&server, "/rest/api/3/mypermissions", json!({"permissions":{"CREATE_ISSUES":{"havePermission":true},"ASSIGN_ISSUES":{"havePermission":true},"EDIT_ISSUES":{"havePermission":true},"SCHEDULE_ISSUES":{"havePermission":false}}})).await;
    sprint_page(
        &server,
        json!([{"id":1234,"name":"2026-Platform-S1","state":"active"}]),
    )
    .await;
    let j = ready(&server).await;
    let o = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert_eq!(o.capabilities.sprint.mode, "unsupported");
    assert!(o.defaults.sprint_id.is_none());
    let mut request = create();
    request.sprint_id = Some("1234".into());
    assert_eq!(
        j.create(&request, &pr(), &[], false)
            .await
            .unwrap_err()
            .code,
        "invalid_choice"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn metadata_ambiguity_permission_failures_and_unknown_required_fields_fail_safely() {
    for scenario in [
        "duplicate_field",
        "duplicate_type",
        "required_field",
        "team_uuid",
        "permission_failure",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        match scenario {
            "duplicate_field" => override_get(&server, "/rest/api/3/field", json!([{"id":"customfield_1","name":"AssignedTeam"},{"id":"customfield_2","name":"AssignedTeam"}])).await,
            "duplicate_type" => override_get(&server, "/rest/api/3/issue/createmeta/DEV/issuetypes", json!({"startAt":0,"total":2,"values":[{"id":"1","name":"Task"},{"id":"2","name":"Task"}]})).await,
            "permission_failure" => Mock::given(method("GET")).and(path("/rest/api/3/field")).respond_with(ResponseTemplate::new(403).set_body_string("private raw error")).with_priority(1).mount(&server).await,
            "required_field" => { let mut fields = create_fields(); fields.push(json!({"fieldId":"customfield_42","name":"Private required field","required":true,"schema":{"type":"unknown"},"operations":["set"]})); field_page(&server, 1, fields).await; }
            "team_uuid" => { let mut fields = create_fields(); fields[5]["schema"] = json!({"type":"string","custom":"com.atlassian.jira.plugin.system.customfieldtypes:atlassian-team"}); field_page(&server, 1, fields).await; }
            _ => unreachable!(),
        }
        let j = ready(&server).await;
        let result = j.options("DEV", Purpose::Review, None, "").await;
        match scenario {
            "duplicate_field" | "duplicate_type" => {
                assert_eq!(result.unwrap_err().code, "jira_metadata_ambiguous")
            }
            "permission_failure" => {
                let e = result.unwrap_err();
                assert_eq!(e.code, "jira_discovery_failed");
                assert!(!e.message.contains("private"));
                assert!(j.projects().await.is_empty());
            }
            "required_field" => {
                let o = result.unwrap();
                assert!(!o.capabilities.can_create);
                assert_eq!(
                    o.capabilities.blockers[0].code,
                    "required_field_unsupported"
                );
                assert_eq!(
                    j.create(&create(), &pr(), &[], true)
                        .await
                        .unwrap_err()
                        .code,
                    "create_blocked"
                );
            }
            "team_uuid" => {
                let o = result.unwrap();
                assert_eq!(o.capabilities.team.mode, "unsupported");
                assert!(o.teams.is_empty());
            }
            _ => unreachable!(),
        }
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method == "GET")
        );
    }
}

#[tokio::test]
async fn failed_refresh_preserves_complete_options_and_submission_does_not_rediscover() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let j = ready(&server).await;
    let o = j.options("DEV", Purpose::Review, None, "").await.unwrap();
    assert!(o.capabilities.can_create);
    Mock::given(method("GET"))
        .and(path("/rest/api/3/field"))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(1)
        .mount(&server)
        .await;
    assert_eq!(
        j.refresh_metadata().await.unwrap_err().code,
        "jira_discovery_failed"
    );
    let before = server.received_requests().await.unwrap().len();
    assert_eq!(
        j.create(&create(), &pr(), &[], true).await.unwrap().outcome,
        "simulated"
    );
    assert!(
        j.options("DEV", Purpose::Review, None, "")
            .await
            .unwrap()
            .capabilities
            .can_create
    );
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(j.projects().await[0].key, "DEV");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

async fn page(
    server: &MockServer,
    p: &str,
    parameter: (&str, &str),
    start: u64,
    total: u64,
    values: Value,
) {
    Mock::given(method("GET"))
        .and(path(p))
        .and(query_param(parameter.0, parameter.1))
        .and(query_param("startAt", start.to_string()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"startAt":start,"maxResults":1,"total":total,"values":values}),
            ),
        )
        .mount(server)
        .await;
}
#[tokio::test]
async fn groups_resolve_real_ids_union_dedup_multipage_intersect_assignable_and_purpose() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let mut inactive = jira_user("inactive");
    inactive["active"] = json!(false);
    let mut app = jira_user("app");
    app["accountType"] = json!("app");
    page(
        &server,
        "/rest/api/3/group/bulk",
        ("groupName", "first"),
        0,
        2,
        json!([{"name":"not-exact","groupId":"no"}]),
    )
    .await;
    page(
        &server,
        "/rest/api/3/group/bulk",
        ("groupName", "first"),
        1,
        2,
        json!([{"name":"first","groupId":"group-A"}]),
    )
    .await;
    page(
        &server,
        "/rest/api/3/group/bulk",
        ("groupName", "second"),
        0,
        1,
        json!([{"name":"second","groupId":"group-B"}]),
    )
    .await;
    let members = [
        jira_user("me"),
        jira_user("other"),
        inactive.clone(),
        app.clone(),
        jira_user("not-assignable"),
    ];
    for (start, member) in members.iter().enumerate() {
        page(
            &server,
            "/rest/api/3/group/member",
            ("groupId", "group-A"),
            start as u64,
            5,
            json!([member]),
        )
        .await;
    }
    page(
        &server,
        "/rest/api/3/group/member",
        ("groupId", "group-B"),
        0,
        1,
        json!([jira_user("other")]),
    )
    .await;
    let mut c = jira(&server.uri());
    c.assignee_groups = vec!["first".into(), "second".into(), "first".into()];
    let j = Jira::new(&c, &Upstream::default()).await.unwrap();
    ready_jira(&j).await;
    let implementation = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert_eq!(
        implementation
            .assignees
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["me", "other"]
    );
    assert!(implementation.defaults.assignee_id.is_none());
    let log = server.received_requests().await.unwrap();
    assert!(
        !log.iter()
            .any(|r| r.url.path() == "/rest/api/3/users/search")
    );
    for account in ["me", "other", "not-assignable"] {
        assert_eq!(
            log.iter()
                .filter(|r| r.url.path() == "/rest/api/3/user/assignable/search"
                    && r.url
                        .query_pairs()
                        .any(|(k, v)| k == "accountId" && v == account))
                .count(),
            1
        );
    }
    let review = j
        .options("DEV", Purpose::Review, None, "OTHER")
        .await
        .unwrap();
    assert_eq!(
        review
            .assignees
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["other"]
    );
    assert!(review.defaults.assignee_id.is_none());
    let mut request = create();
    request.assignee_id = Some("me".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );
    request.assignee_id = Some("outside-group".into());
    request.purpose = Purpose::Implementation;
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn group_missing_ambiguous_forbidden_middle_page_and_bounds_never_fall_back() {
    for scenario in [
        "missing",
        "ambiguous",
        "forbidden",
        "middle",
        "bounds",
        "offset",
    ] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let values = match scenario {
            "missing" => json!([]),
            "ambiguous" => {
                json!([{"name":"restricted","groupId":"a"},{"name":"restricted","groupId":"b"}])
            }
            _ => json!([{"name":"restricted","groupId":"a"}]),
        };
        mock_get(
            &server,
            "/rest/api/3/group/bulk",
            json!({"startAt":0,"total":values.as_array().unwrap().len(),"values":values}),
        )
        .await;
        if scenario == "forbidden" {
            Mock::given(method("GET"))
                .and(path("/rest/api/3/group/member"))
                .respond_with(ResponseTemplate::new(403))
                .mount(&server)
                .await;
        } else {
            page(
                &server,
                "/rest/api/3/group/member",
                ("groupId", "a"),
                0,
                if scenario == "bounds" { 100 } else { 2 },
                json!([jira_user("me")]),
            )
            .await;
            if scenario == "offset" {
                Mock::given(method("GET"))
                    .and(path("/rest/api/3/group/member"))
                    .and(query_param("startAt", "1"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(
                        json!({"startAt":0,"total":2,"values":[jira_user("other")]}),
                    ))
                    .mount(&server)
                    .await;
            }
        }
        let mut c = jira(&server.uri());
        c.assignee_groups = vec!["restricted".into()];
        let j = Jira::new(
            &c,
            &Upstream {
                max_pages: if scenario == "bounds" { 1 } else { 10 },
                ..Default::default()
            },
        )
        .await
        .unwrap();
        j.identity().await.unwrap();
        assert_eq!(
            j.discover_assignees("DEV", Purpose::Implementation, "")
                .await
                .unwrap_err()
                .code,
            "jira_discovery_failed"
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method == "GET")
        );
    }
}

#[tokio::test]
async fn create_metadata_types_and_fields_paginate_and_fail_on_incomplete_page() {
    for incomplete in [false, true] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        for (start, id, name) in [(0, "1", "Task"), (1, "2", "Story")] {
            Mock::given(method("GET")).and(path("/rest/api/3/issue/createmeta/DEV/issuetypes")).and(query_param("startAt",start.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"startAt":start,"maxResults":1,"total":2,"issueTypes":[{"id":id,"name":name}]}))).with_priority(1).mount(&server).await;
        }
        for id in [1, 2] {
            let fields = create_fields();
            for (start, field) in fields.iter().enumerate() {
                if incomplete && id == 1 && start == 1 {
                    continue;
                }
                Mock::given(method("GET")).and(path(format!("/rest/api/3/issue/createmeta/DEV/issuetypes/{id}"))).and(query_param("startAt",start.to_string()))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"startAt":start,"maxResults":1,"total":fields.len(),"fields":[field]}))).with_priority(1).mount(&server).await;
            }
        }
        let j = ready(&server).await;
        let o = j.options("DEV", Purpose::Review, None, "").await;
        if incomplete {
            assert_eq!(o.unwrap_err().code, "jira_discovery_failed");
            assert!(j.projects().await.is_empty());
        } else {
            let o = o.unwrap();
            assert_eq!(o.issue_types, ["Story", "Task"]);
            assert_eq!(o.capabilities.team.mode, "create");
        }
    }
}

#[tokio::test]
async fn unavailable_or_ambiguous_team_default_requires_explicit_choice_not_first_option() {
    for duplicate in [false, true] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        let mut fields = create_fields();
        fields[5]["allowedValues"] = if duplicate {
            json!([{"id":"1","value":"Platform"},{"id":"2","value":"Platform"}])
        } else {
            json!([{"id":"1","value":"Other"}])
        };
        field_page(&server, 1, fields).await;
        let j = ready(&server).await;
        let o = j.options("DEV", Purpose::Review, None, "").await.unwrap();
        assert!(o.defaults.team.is_none());
        assert!(
            !o.diagnostics
                .iter()
                .any(|d| d.code.starts_with("team_default_"))
        );
        assert!(o.capabilities.can_create);
        assert!(!o.teams.is_empty());
    }
}

#[tokio::test]
async fn individual_assignability_mismatched_id_or_permission_failure_never_returns_partial_pool() {
    for repeat in [false, true] {
        let server = MockServer::start().await;
        mock_jira(&server).await;
        if repeat {
            Mock::given(method("GET"))
                .and(path("/rest/api/3/user/assignable/search"))
                .and(query_param("accountId", "other"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([jira_user("me")])))
                .with_priority(1)
                .mount(&server)
                .await;
        } else {
            Mock::given(method("GET"))
                .and(path("/rest/api/3/user/assignable/search"))
                .and(query_param("accountId", "other"))
                .respond_with(ResponseTemplate::new(403))
                .with_priority(1)
                .mount(&server)
                .await;
        }
        let j = ready(&server).await;
        assert_eq!(
            j.discover_assignees("DEV", Purpose::Implementation, "")
                .await
                .unwrap_err()
                .code,
            "jira_discovery_failed"
        );
    }
}

#[tokio::test]
async fn changed_group_membership_takes_effect_only_after_successful_refresh_without_me_default() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    page(
        &server,
        "/rest/api/3/group/bulk",
        ("groupName", "restricted"),
        0,
        1,
        json!([{"name":"restricted","groupId":"real-id"}]),
    )
    .await;
    page(
        &server,
        "/rest/api/3/group/member",
        ("groupId", "real-id"),
        0,
        1,
        json!([jira_user("me")]),
    )
    .await;
    let mut c = jira(&server.uri());
    c.assignee_groups = vec!["restricted".into()];
    let j = Jira::new(&c, &Upstream::default()).await.unwrap();
    ready_jira(&j).await;
    let o = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert!(o.defaults.assignee_id.is_none());
    override_get(
        &server,
        "/rest/api/3/group/member",
        json!({"startAt":0,"total":1,"values":[jira_user("other")]}),
    )
    .await;
    let mut request = create();
    request.purpose = Purpose::Implementation;
    request.assignee_id = Some("me".into());
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap().outcome,
        "simulated"
    );
    j.refresh_metadata().await.unwrap();
    assert_eq!(
        j.create(&request, &pr(), &[], true).await.unwrap_err().code,
        "invalid_choice"
    );
    let o = j
        .options("DEV", Purpose::Implementation, None, "")
        .await
        .unwrap();
    assert!(o.defaults.assignee_id.is_none());
    assert!(
        !o.diagnostics
            .iter()
            .any(|d| d.code == "assignee_default_ineligible")
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn concurrent_options_read_only_committed_snapshot_and_failed_refresh_retains_it() {
    let server = MockServer::start().await;
    mock_jira(&server).await;
    let j = ready(&server).await;
    let (a, b) = tokio::join!(
        j.options("DEV", Purpose::Implementation, None, ""),
        j.options("DEV", Purpose::Review, None, "")
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/rest/api/3/field")
            .count(),
        1
    );
    Mock::given(method("GET"))
        .and(path("/rest/api/3/field"))
        .respond_with(ResponseTemplate::new(403))
        .with_priority(1)
        .mount(&server)
        .await;
    assert_eq!(
        j.refresh_metadata().await.unwrap_err().code,
        "jira_discovery_failed"
    );
    assert!(
        j.options("DEV", Purpose::Review, None, "")
            .await
            .unwrap()
            .capabilities
            .can_create
    );
    assert_eq!(j.projects().await[0].key, "DEV");
}
