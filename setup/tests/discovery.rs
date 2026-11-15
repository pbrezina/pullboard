use anyhow::Result;
use pullboard_core::{
    config::{AiConfig, AiProvider, Config, ForgeConfig, JiraConfig, Secret, Upstream},
    model::Kind,
};
use pullboard_setup::discovery::{self, GetJson, JiraClient};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

type RecordedRequest = (String, Vec<(String, String)>);

struct Fake {
    responses: Mutex<VecDeque<(&'static str, Value)>>,
    requests: Mutex<Vec<RecordedRequest>>,
    limits: Upstream,
}
impl Fake {
    fn new(values: Vec<(&'static str, Value)>) -> Self {
        Self {
            responses: Mutex::new(values.into()),
            requests: Mutex::default(),
            limits: Upstream::default(),
        }
    }
}
impl GetJson for Fake {
    async fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        self.requests.lock().unwrap().push((
            path.into(),
            params
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        ));
        let (expected, value) = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra read");
        assert_eq!(path, expected);
        Ok(value)
    }
    fn limits(&self) -> &Upstream {
        &self.limits
    }
}
#[tokio::test]
async fn pages_follow_complete_inventory() {
    let fake = Fake::new(vec![
        (
            "pool",
            json!({"startAt":0,"total":3,"values":[{"id":"a"},{"id":"b"}]}),
        ),
        ("pool", json!({"startAt":2,"total":3,"values":[{"id":"c"}]})),
    ]);
    assert_eq!(
        discovery::pages(&fake, "pool", &[], &["values"])
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn pages_retain_promised_total_when_later_last_page_omits_it() {
    for last_values in [json!([{"id":"b"}]), json!([])] {
        let fake = Fake::new(vec![
            ("pool", json!({"startAt":0,"total":3,"values":[{"id":"a"}]})),
            (
                "pool",
                json!({"startAt":1,"isLast":true,"values":last_values}),
            ),
        ]);
        assert!(
            discovery::pages(&fake, "pool", &[], &["values"])
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn pages_retain_total_bounds_when_later_page_omits_it() {
    let fake = Fake::new(vec![
        ("pool", json!({"startAt":0,"total":2,"values":[1]})),
        ("pool", json!({"startAt":1,"isLast":true,"values":[2,3]})),
    ]);
    assert!(
        discovery::pages(&fake, "pool", &[], &["values"])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pages_accept_honest_omitted_total_after_all_promised_rows() {
    for last in [
        json!({"startAt":1,"isLast":true,"values":[2,3]}),
        json!({"startAt":1,"values":[2,3]}),
    ] {
        let fake = Fake::new(vec![
            ("pool", json!({"startAt":0,"total":3,"values":[1]})),
            ("pool", last),
        ]);
        assert_eq!(
            discovery::pages(&fake, "pool", &[], &["values"])
                .await
                .unwrap()
                .len(),
            3
        );
    }
}

#[tokio::test]
async fn pages_without_any_total_require_explicit_completion() {
    let empty = Fake::new(vec![("pool", json!({"startAt":0,"values":[]}))]);
    assert!(
        discovery::pages(&empty, "pool", &[], &["values"])
            .await
            .is_err()
    );
    let honest_empty = Fake::new(vec![(
        "pool",
        json!({"startAt":0,"isLast":true,"values":[]}),
    )]);
    assert!(
        discovery::pages(&honest_empty, "pool", &[], &["values"])
            .await
            .unwrap()
            .is_empty()
    );
    let honest = Fake::new(vec![
        ("pool", json!({"startAt":0,"isLast":false,"values":[1]})),
        ("pool", json!({"startAt":1,"isLast":true,"values":[2]})),
    ]);
    assert_eq!(
        discovery::pages(&honest, "pool", &[], &["values"])
            .await
            .unwrap()
            .len(),
        2
    );
}
#[tokio::test]
async fn pages_fail_closed_on_repetition_wrong_offsets_total_changes_and_bound_exhaustion() {
    for values in [
        vec![
            json!({"startAt":0,"values":[1]}),
            json!({"startAt":1,"values":[1]}),
        ],
        vec![json!({"startAt":7,"total":1,"values":[1]})],
        vec![
            json!({"startAt":0,"total":3,"values":[1]}),
            json!({"startAt":1,"total":4,"values":[2]}),
        ],
        vec![json!({"startAt":0,"total":2,"values":[]})],
        vec![json!({"startAt":0,"isLast":true,"total":2,"values":[1]})],
    ] {
        let fake = Fake::new(values.into_iter().map(|v| ("pool", v)).collect());
        assert!(
            discovery::pages(&fake, "pool", &[], &["values"])
                .await
                .is_err()
        );
    }
    let mut fake = Fake::new(vec![("pool", json!({"startAt":0,"values":[1]}))]);
    fake.limits.max_pages = 1;
    assert!(
        discovery::pages(&fake, "pool", &[], &["values"])
            .await
            .is_err()
    );
}
#[tokio::test]
async fn all_groups_does_not_treat_partial_pools_as_empty_success() {
    let fake = Fake::new(vec![(
        "rest/api/3/group/bulk",
        json!({"startAt":0,"total":1,"values":[]}),
    )]);
    assert!(discovery::all_groups_from(&fake).await.is_err());
}

#[tokio::test]
async fn default_groups_use_only_verified_self_and_nonpaginated_memberships() {
    let fake = Fake::new(vec![
        ("rest/api/3/myself", me()),
        (
            "rest/api/3/user/groups",
            json!([
                {"groupId":"g1","name":"developers"},
                {"groupId":"g2","name":"team"},
            ]),
        ),
    ]);
    let groups = discovery::own_groups_from(&fake).await.unwrap();
    assert_eq!(
        groups.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["developers", "team"]
    );
    assert_eq!(
        *fake.requests.lock().unwrap(),
        [
            ("rest/api/3/myself".into(), vec![]),
            (
                "rest/api/3/user/groups".into(),
                vec![("accountId".into(), "actual-id".into())]
            ),
        ]
    );
}

#[tokio::test]
async fn own_groups_reuse_scoped_verified_identity_without_scanning_tenant_groups() {
    let server = MockServer::start().await;
    mock_json(&server, "/rest/api/3/myself", me(), 1).await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/user/groups"))
        .and(query_param("accountId", "actual-id"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"name":"developers","groupId":"g1"}])),
        )
        .expect(2)
        .mount(&server)
        .await;
    let client = JiraClient::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    client.get_json("rest/api/3/myself", &[]).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            discovery::own_groups_from(&client).await.unwrap()[0].id,
            "developers"
        );
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|r| r.method == "GET"));
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path() == "/rest/api/3/group/bulk")
    );
}

#[tokio::test]
async fn own_groups_reject_bad_identity_malformed_arrays_and_ambiguous_names() {
    for identity in [
        json!({"active":true}),
        json!({"accountId":"app","active":true,"accountType":"app"}),
        json!({"accountId":"inactive","active":false}),
    ] {
        let fake = Fake::new(vec![("rest/api/3/myself", identity)]);
        assert!(discovery::own_groups_from(&fake).await.is_err());
    }
    for groups in [
        json!({"values":[]}),
        json!([{}]),
        json!([{"name":"same","groupId":"a"},{"name":"same","groupId":"b"}]),
    ] {
        let fake = Fake::new(vec![
            ("rest/api/3/myself", me()),
            ("rest/api/3/user/groups", groups),
        ]);
        assert!(discovery::own_groups_from(&fake).await.is_err());
    }
    let fake = Fake::new(vec![
        ("rest/api/3/myself", me()),
        ("rest/api/3/user/groups", json!([])),
    ]);
    assert!(discovery::own_groups_from(&fake).await.unwrap().is_empty());
}

#[tokio::test]
async fn manual_groups_use_exact_name_scoping_and_complete_member_read_verification() {
    let fake = Fake::new(vec![
        (
            "rest/api/3/group/bulk",
            json!({"startAt":0,"total":1,"values":[{"name":"external team","groupId":"g"}]}),
        ),
        (
            "rest/api/3/group/member",
            json!({"startAt":0,"total":2,"values":[{"accountId":"one"}]}),
        ),
        (
            "rest/api/3/group/member",
            json!({"startAt":1,"total":2,"values":[{"accountId":"two"}]}),
        ),
    ]);
    assert_eq!(
        discovery::group_from(&fake, "external team")
            .await
            .unwrap()
            .id,
        "external team"
    );
    let requests = fake.requests.lock().unwrap();
    assert!(
        requests[0]
            .1
            .contains(&("groupName".into(), "external team".into()))
    );
    for (_, params) in &requests[1..] {
        assert!(params.contains(&("groupname".into(), "external team".into())));
        assert!(params.contains(&("includeInactiveUsers".into(), "false".into())));
    }
}

#[tokio::test]
async fn manual_groups_fail_closed_for_wrong_names_and_incomplete_members() {
    for rows in [
        json!([]),
        json!([{"name":"wrong"}]),
        json!([{"name":"wanted"},{"name":"wanted"}]),
    ] {
        let fake = Fake::new(vec![(
            "rest/api/3/group/bulk",
            json!({"startAt":0,"total":rows.as_array().unwrap().len(),"values":rows}),
        )]);
        assert!(discovery::group_from(&fake, "wanted").await.is_err());
    }
    let fake = Fake::new(vec![
        (
            "rest/api/3/group/bulk",
            json!({"startAt":0,"total":1,"values":[{"name":"wanted"}]}),
        ),
        (
            "rest/api/3/group/member",
            json!({"startAt":0,"total":1,"values":[]}),
        ),
    ]);
    assert!(discovery::group_from(&fake, "wanted").await.is_err());
}
#[tokio::test]
async fn metadata_verifies_groups_and_keeps_active_future_and_only_unique_status_names() {
    let config = Config::from_yaml("jira:\n  base_url: https://example.atlassian.net\n  email: fake@example.invalid\n  token: {value: fake}\n  projects: [ONE]\n  board_id: 7\n  team: Actual team\n  sprint_name_pattern: Team\n  assignee_groups: [developers]\n").unwrap();
    let fake = Fake::new(vec![
        (
            "rest/api/3/group/member",
            json!({"startAt":0,"total":0,"values":[]}),
        ),
        (
            "rest/api/3/field",
            json!([{"id":"customfield_1","name":"AssignedTeam"}]),
        ),
        ("rest/agile/1.0/board/7", json!({"id":7})),
        (
            "rest/agile/1.0/board/7/sprint",
            json!({"startAt":0,"isLast":true,"values":[{"id":1,"name":"Foreign active","state":"active"},{"id":2,"name":"Team future","state":"future"}]}),
        ),
        (
            "rest/api/3/issue/createmeta/ONE/issuetypes",
            json!({"startAt":0,"total":1,"issueTypes":[{"id":"10","name":"Task"}]}),
        ),
        (
            "rest/api/3/project/ONE/statuses",
            json!([{"id":"10","name":"Task","statuses":[{"id":"1","name":"Open"},{"id":"2","name":"Duplicate"},{"id":"3","name":"Duplicate"}]}]),
        ),
        (
            "rest/api/3/issue/createmeta/ONE/issuetypes/10",
            json!({"startAt":0,"total":1,"fields":[{"fieldId":"customfield_1","name":"AssignedTeam","operations":["set"],"schema":{"type":"option","custom":"com.atlassian.jira.plugin.system.customfieldtypes:select"},"allowedValues":[{"id":"100","value":"Actual team"},{"id":"101","value":"Disabled team","disabled":true}]}]}),
        ),
    ]);
    let m = discovery::metadata_from(&fake, config.jira.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(m.sprints.len(), 2);
    assert_eq!(m.sprints[0].id, "1");
    assert_eq!(m.team_field_id.as_deref(), Some("customfield_1"));
    assert_eq!(m.types[0].team_options[0].id, "100");
    assert_eq!(m.types[0].teams, ["Actual team"]);
    assert_eq!(m.types[0].statuses, ["Open"]);
    assert!(fake.responses.lock().unwrap().is_empty());
}
#[tokio::test]
async fn forge_identity_uses_shared_read_only_adapter_auth_and_versioned_base() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .and(header("private-token", "synthetic-test-token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":17,"username":"actual-login"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = ForgeConfig {
        id: "gitlab-1".into(),
        name: None,
        kind: Kind::Gitlab,
        base_url: server.uri(),
        token: Secret::Literal {
            value: "synthetic-test-token".into(),
        },
        include_repositories: vec![],
        exclude_repositories: vec![],
        default_project: None,
    };
    assert_eq!(
        discovery::forge_identity(&f, &Upstream::default())
            .await
            .unwrap(),
        "actual-login"
    );
}
#[tokio::test]
async fn synthetic_inference_uses_exact_model_and_only_fixed_synthetic_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_json(json!({"model":"gpt-6-luna","messages":[{"role":"user","content":"Reply with the word ready. This is a synthetic PullBoard setup test."}],"max_completion_tokens":16}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"ready"}}]}))).expect(1).mount(&server).await;
    let ai = AiConfig {
        enabled: true,
        provider: AiProvider::Openai,
        model: "gpt-6-luna".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        token: Some(Secret::Literal {
            value: "synthetic-test-token".into(),
        }),
    };
    discovery::synthetic_ai_test(&ai, &Upstream::default())
        .await
        .unwrap();
}

fn jira(base: &str) -> JiraConfig {
    let mut jira = Config::from_yaml("jira:\n  base_url: https://example.atlassian.net\n  email: fake@example.invalid\n  token: {value: synthetic-test-token}\n  projects: [ONE]\n  board_id: 7\n  team: Actual team\n  sprint_name_pattern: Team\n  assignee_groups: [developers]\n").unwrap().jira.unwrap();
    jira.base_url = base.into();
    jira
}

fn me() -> Value {
    json!({"accountId":"actual-id","displayName":"Actual Human","active":true,"accountType":"atlassian"})
}

#[tokio::test]
async fn initial_catalog_and_boards_use_only_exact_recent_endpoints_and_leave_settings_empty() {
    let fake = Fake::new(vec![
        ("rest/api/3/myself", me()),
        (
            "rest/api/3/project/recent",
            json!([{"id":"11","key":"ONE","name":"One project"}]),
        ),
        (
            "rest/greenhopper/1.0/rapidviews/recent",
            json!({"views":[{"id":7,"name":"Team board"}]}),
        ),
    ]);
    let catalog = discovery::catalog_from(&fake).await.unwrap();
    assert_eq!(catalog.identity, "Actual Human");
    assert_eq!(catalog.projects[0].id, "ONE");
    assert!(catalog.groups.is_empty());
    assert!(catalog.fields.is_empty());
    assert_eq!(discovery::boards_from(&fake).await.unwrap()[0].id, "7");
    assert!(fake.responses.lock().unwrap().is_empty());
    assert!(
        fake.requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, params)| params.is_empty())
    );
}

#[tokio::test]
async fn empty_recents_are_honest_empty_lists_without_full_scan_fallback() {
    let fake = Fake::new(vec![
        ("rest/api/3/myself", me()),
        ("rest/api/3/project/recent", json!([])),
        (
            "rest/greenhopper/1.0/rapidviews/recent",
            json!({"views":[]}),
        ),
    ]);
    assert!(
        discovery::catalog_from(&fake)
            .await
            .unwrap()
            .projects
            .is_empty()
    );
    assert!(discovery::boards_from(&fake).await.unwrap().is_empty());
    assert_eq!(fake.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn selected_project_board_inventory_completes_pagination_and_deduplicates_shared_boards() {
    let fake = Fake::new(vec![
        (
            "rest/agile/1.0/board",
            json!({"startAt":0,"total":2,"values":[{"id":7,"name":"Shared"}]}),
        ),
        (
            "rest/agile/1.0/board",
            json!({"startAt":1,"total":2,"values":[{"id":8,"name":"First only"}]}),
        ),
        (
            "rest/agile/1.0/board",
            json!({"startAt":0,"isLast":true,"values":[{"id":7,"name":"Shared"},{"id":9,"name":"Second only"}]}),
        ),
    ]);
    let mut config = jira("https://example.atlassian.net");
    config.projects.push("TWO".into());
    let boards = discovery::project_boards_from(&fake, &config)
        .await
        .unwrap();
    assert_eq!(
        boards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["7", "8", "9"]
    );
    {
        let requests = fake.requests.lock().unwrap();
        for ((path, query), (project, offset)) in
            requests
                .iter()
                .zip([("ONE", "0"), ("ONE", "1"), ("TWO", "0")])
        {
            assert_eq!(path, "rest/agile/1.0/board");
            assert!(query.contains(&("projectKeyOrId".into(), project.into())));
            assert!(query.contains(&("startAt".into(), offset.into())));
            assert!(query.contains(&("maxResults".into(), "100".into())));
        }
    }
    let empty = Fake::new(vec![]);
    config.projects.clear();
    assert!(
        discovery::project_boards_from(&empty, &config)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(empty.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_all_projects_and_exact_name_search_are_complete_and_names_require_exact_matches()
{
    let fake = Fake::new(vec![
        (
            "rest/api/3/project/search",
            json!({"startAt":0,"total":2,"values":[{"key":"ONE","name":"Common name"}]}),
        ),
        (
            "rest/api/3/project/search",
            json!({"startAt":1,"total":2,"values":[{"key":"TWO","name":"Other"}]}),
        ),
        (
            "rest/api/3/project/search",
            json!({"startAt":0,"total":3,"values":[{"key":"ONE","name":"Common name"},{"key":"TWO","name":"Common name"},{"key":"THREE","name":"Common name extra"}]}),
        ),
    ]);
    assert_eq!(discovery::all_projects_from(&fake).await.unwrap().len(), 2);
    let matches =
        discovery::project_from(&fake, &jira("https://example.atlassian.net"), "Common name")
            .await
            .unwrap();
    assert_eq!(
        matches.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["ONE", "TWO"]
    );
    let requests = fake.requests.lock().unwrap();
    assert!(!requests[0].1.iter().any(|(k, _)| k == "query"));
    assert!(
        requests[2]
            .1
            .contains(&("query".into(), "Common name".into()))
    );
}

#[tokio::test]
async fn exact_name_lookup_does_not_accept_case_insensitive_or_substring_results() {
    let fake = Fake::new(vec![(
        "rest/api/3/project/search",
        json!({"startAt":0,"total":2,"values":[{"key":"ONE","name":"common name"},{"key":"TWO","name":"Common name extra"}]}),
    )]);
    let error =
        discovery::project_from(&fake, &jira("https://example.atlassian.net"), "Common name")
            .await
            .unwrap_err();
    assert!(error.to_string().contains("exact name"));
}

#[tokio::test]
async fn exact_project_names_can_contain_normal_punctuation_without_becoming_browser_urls() {
    for name in [
        "Platform / API: v2",
        "Growth 100%",
        "CI/CD",
        "Team @ Work",
        "Team: Alpha",
    ] {
        let fake = Fake::new(vec![(
            "rest/api/3/project/search",
            json!({"startAt":0,"total":1,"values":[{"key":"ONE","name":name}]}),
        )]);
        assert_eq!(
            discovery::project_from(&fake, &jira("https://example.atlassian.net"), name)
                .await
                .unwrap()[0]
                .id,
            "ONE"
        );
        assert!(
            fake.requests.lock().unwrap()[0]
                .1
                .contains(&("query".into(), name.into()))
        );
    }
}

#[tokio::test]
async fn submitted_project_keys_and_supported_project_urls_construct_only_direct_api_paths() {
    let config = jira("https://example.atlassian.net");
    for input in [
        "ONE",
        "https://example.atlassian.net/projects/ONE",
        "https://example.atlassian.net/projects/ONE/summary/",
        "https://example.atlassian.net/browse/ONE",
        "https://example.atlassian.net/jira/projects/ONE",
        "https://example.atlassian.net/jira/software/projects/ONE",
        "https://example.atlassian.net/jira/software/projects/ONE/issues",
        "https://example.atlassian.net/jira/software/c/projects/ONE/summary",
        "https://example.atlassian.net/jira/software/c/projects/ONE/issues",
        "https://example.atlassian.net/jira/core/projects/ONE/list",
        "https://example.atlassian.net/jira/servicedesk/projects/ONE/queues",
        "https://example.atlassian.net/jira/software/c/projects/ONE/boards/7",
        "https://example.atlassian.net/plugins/servlet/project-config/ONE/details",
    ] {
        let fake = Fake::new(vec![(
            "rest/api/3/project/ONE",
            json!({"key":"ONE","name":"One"}),
        )]);
        assert_eq!(
            discovery::project_from(&fake, &config, input)
                .await
                .unwrap()[0]
                .id,
            "ONE"
        );
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
    }
    let config = jira("https://example.atlassian.net/context");
    let fake = Fake::new(vec![(
        "rest/api/3/project/ONE",
        json!({"key":"ONE","name":"One"}),
    )]);
    discovery::project_from(
        &fake,
        &config,
        "https://example.atlassian.net/context/projects/ONE",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn submitted_board_ids_and_supported_board_urls_verify_response_identity_once() {
    let config = jira("https://example.atlassian.net");
    for input in [
        "7",
        "https://example.atlassian.net/boards/7",
        "https://example.atlassian.net/jira/boards/7",
        "https://example.atlassian.net/jira/software/c/projects/ONE/boards/7",
        "https://example.atlassian.net/jira/software/projects/ONE/boards/7/backlog?selectedSprint=9",
        "https://example.atlassian.net/secure/RapidBoard.jspa?rapidView=7",
        "https://example.atlassian.net/secure/RapidBoard.jspa?projectKey=ONE&rapidView=7",
        "https://example.atlassian.net/secure/RapidBoard.jspa?view=planning.nodetail&selectedSprint=9&rapidView=7",
    ] {
        let fake = Fake::new(vec![(
            "rest/agile/1.0/board/7",
            json!({"id":7,"name":"Board"}),
        )]);
        assert_eq!(
            discovery::board_from(&fake, &config, input)
                .await
                .unwrap()
                .id,
            "7"
        );
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
    }
    let fake = Fake::new(vec![(
        "rest/agile/1.0/board/7",
        json!({"id":8,"name":"Board"}),
    )]);
    assert!(
        discovery::board_from(&fake, &config, "7")
            .await
            .unwrap_err()
            .to_string()
            .contains("different board ID")
    );
    let fake = Fake::new(vec![(
        "rest/api/3/project/ONE",
        json!({"key":"TWO","name":"Other"}),
    )]);
    assert!(
        discovery::project_from(&fake, &config, "ONE")
            .await
            .unwrap_err()
            .to_string()
            .contains("different project key")
    );
}

#[tokio::test]
async fn unsafe_or_ambiguous_urls_are_rejected_before_any_request() {
    let config = jira("https://example.atlassian.net");
    let fake = Fake::new(vec![]);
    for input in [
        "https://other.atlassian.net/projects/ONE",
        "https://example.atlassian.net.evil.invalid/projects/ONE",
        "http://example.atlassian.net/projects/ONE",
        "https://example.atlassian.net:444/projects/ONE",
        "https://user:password@example.atlassian.net/projects/ONE",
        "https://@example.atlassian.net/projects/ONE",
        "https://example.atlassian.net/projects/../projects/ONE",
        "https://example.atlassian.net/projects/%2e%2e/projects/ONE",
        "https://example.atlassian.net/projects/ONE%2fTWO",
        "https://example.atlassian.net/projects/ONE%252fTWO",
        "https://example.atlassian.net//projects/ONE",
        "https://example.atlassian.net/projects\\ONE",
        "https://example.atlassian.net/projects/ONE#fragment",
        "https://example.atlassian.net/projects/ONE?redirect=https://evil.invalid",
        "https://example.atlassian.net/projects/ONE?projectKey=TWO",
        "https://example.atlassian.net/projects/ONE?rapidView=7",
        "https://example.atlassian.net/secure/RapidBoard.jspa?rapidView=7&rapidView=8",
        "https://example.atlassian.net/secure/RapidBoard.jspa?rapidView=0",
        "https://example.atlassian.net/secure/RapidBoard.jspa?rapidView=7&next=evil",
        "https://example.atlassian.net/secure/RapidBoard.jspa?rapidView=7&view=unknown",
        "https://example.atlassian.net/jira/software/c/projects/ONE/boards/7?rapidView=8",
        "https://example.atlassian.net/boards/7/boards/8",
        "https://example.atlassian.net/browse/ONE-7",
        "https:example.atlassian.net/projects/ONE",
        "https//example.atlassian.net/projects/ONE",
        "https%3A%2F%2Fexample.atlassian.net%2Fprojects%2FONE",
        "example.atlassian.net/projects/ONE",
        "javascript:alert(1)",
        "//evil.invalid/projects/ONE",
        "https://example.atlassian.net/projects/ONE\n",
    ] {
        assert!(
            discovery::project_from(&fake, &config, input)
                .await
                .is_err(),
            "accepted project URL {input}"
        );
        assert!(
            discovery::board_from(&fake, &config, input).await.is_err(),
            "accepted board URL {input}"
        );
    }
    for input in ["0", "-1", "1.5", "18446744073709551616", ""] {
        assert!(discovery::board_from(&fake, &config, input).await.is_err());
    }
    let context = jira("https://example.atlassian.net/context");
    assert!(
        discovery::project_from(
            &fake,
            &context,
            "https://example.atlassian.net/context-other/projects/ONE"
        )
        .await
        .is_err()
    );
    assert!(fake.requests.lock().unwrap().is_empty());
}

async fn mock_json(server: &MockServer, route: &str, value: Value, count: u64) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .expect(count)
        .mount(server)
        .await;
}

#[tokio::test]
async fn key_shaped_names_fall_back_to_filtered_exact_search_only_after_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/project/ALPHA"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/project/search"))
        .and(query_param("query", "ALPHA"))
        .and(query_param("startAt", "0"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"startAt":0,"total":1,"values":[{"key":"ONE","name":"ALPHA"}]}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    assert_eq!(
        discovery::project_from(&client, &config, "ALPHA")
            .await
            .unwrap()[0]
            .id,
        "ONE"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn failed_key_lookups_do_not_search_on_permission_failure_or_url_not_found() {
    for (status, use_url) in [(401, false), (403, false), (503, false), (404, true)] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/project/ONE"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        let config = jira(&server.uri());
        let client = JiraClient::new(&config, &Upstream::default())
            .await
            .unwrap();
        let input = if use_url {
            format!("{}/projects/ONE", server.uri())
        } else {
            "ONE".into()
        };
        let error = discovery::project_from(&client, &config, &input)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains(&format!("HTTP {status}")));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn successful_list_and_direct_entries_and_field_reads_are_reused_without_hydration() {
    let server = MockServer::start().await;
    mock_json(&server, "/rest/api/3/myself", me(), 1).await;
    mock_json(
        &server,
        "/rest/api/3/project/recent",
        json!([{"id":"11","key":"ONE","name":"One"}]),
        1,
    )
    .await;
    mock_json(
        &server,
        "/rest/greenhopper/1.0/rapidviews/recent",
        json!({"views":[{"id":7,"name":"Recent board"}]}),
        1,
    )
    .await;
    mock_json(
        &server,
        "/rest/api/3/project/TWO",
        json!({"key":"TWO","name":"Manual"}),
        1,
    )
    .await;
    mock_json(
        &server,
        "/rest/agile/1.0/board/8",
        json!({"id":8,"name":"Manual board"}),
        1,
    )
    .await;
    mock_json(&server, "/rest/api/3/field", json!([]), 1).await;
    mock_json(
        &server,
        "/rest/api/3/group/member",
        json!({"startAt":0,"total":0,"values":[]}),
        1,
    )
    .await;
    mock_json(
        &server,
        "/rest/agile/1.0/board/7/sprint",
        json!({"startAt":0,"isLast":true,"values":[]}),
        1,
    )
    .await;
    mock_json(
        &server,
        "/rest/api/3/issue/createmeta/ONE/issuetypes",
        json!({"startAt":0,"total":0,"issueTypes":[]}),
        1,
    )
    .await;
    mock_json(&server, "/rest/api/3/project/ONE/statuses", json!([]), 1).await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    for _ in 0..2 {
        discovery::catalog_from(&client).await.unwrap();
        discovery::boards_from(&client).await.unwrap();
        discovery::project_from(&client, &config, "ONE")
            .await
            .unwrap();
        discovery::project_from(&client, &config, "TWO")
            .await
            .unwrap();
        discovery::board_from(&client, &config, "7").await.unwrap();
        discovery::board_from(&client, &config, "8").await.unwrap();
    }
    discovery::settings_from(&client).await.unwrap();
    discovery::metadata_from(&client, &config).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 10);
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(
        !requests
            .iter()
            .any(|request| request.url.path() == "/rest/api/3/project/ONE"
                || request.url.path() == "/rest/agile/1.0/board/7")
    );
}

#[tokio::test]
async fn explicit_complete_inventory_rows_are_reused_but_inventory_pages_are_not_cached() {
    let server = MockServer::start().await;
    mock_json(
        &server,
        "/rest/api/3/project/search",
        json!({"startAt":0,"total":1,"values":[{"key":"ONE","name":"One"}]}),
        2,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/rest/agile/1.0/board"))
        .and(query_param("projectKeyOrId", "ONE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"startAt":0,"total":1,"values":[{"id":7,"name":"Scoped board"}]}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    for _ in 0..2 {
        discovery::all_projects_from(&client).await.unwrap();
        discovery::project_boards_from(&client, &config)
            .await
            .unwrap();
        discovery::project_from(&client, &config, "ONE")
            .await
            .unwrap();
        discovery::board_from(&client, &config, "7").await.unwrap();
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn separate_credential_scoped_clients_never_share_successful_private_responses() {
    let first = MockServer::start().await;
    let second = MockServer::start().await;
    mock_json(
        &first,
        "/rest/agile/1.0/board/7",
        json!({"id":7,"name":"First tenant"}),
        3,
    )
    .await;
    mock_json(
        &second,
        "/rest/agile/1.0/board/7",
        json!({"id":7,"name":"Second tenant"}),
        1,
    )
    .await;
    let original = jira(&first.uri());
    let a = JiraClient::new(&original, &Upstream::default())
        .await
        .unwrap();
    assert!(
        discovery::board_from(&a, &original, "7")
            .await
            .unwrap()
            .label
            .contains("First tenant")
    );
    let mut email_changed = original.clone();
    email_changed.email = "other@example.invalid".into();
    let b = JiraClient::new(&email_changed, &Upstream::default())
        .await
        .unwrap();
    discovery::board_from(&b, &email_changed, "7")
        .await
        .unwrap();
    let mut token_changed = original.clone();
    token_changed.token = Secret::Literal {
        value: "other-synthetic-token".into(),
    };
    let c = JiraClient::new(&token_changed, &Upstream::default())
        .await
        .unwrap();
    discovery::board_from(&c, &token_changed, "7")
        .await
        .unwrap();
    let url_changed = jira(&second.uri());
    let d = JiraClient::new(&url_changed, &Upstream::default())
        .await
        .unwrap();
    assert!(
        discovery::board_from(&d, &url_changed, "7")
            .await
            .unwrap()
            .label
            .contains("Second tenant")
    );
}

#[tokio::test]
async fn successful_read_cache_evicts_old_entries_at_its_fixed_entry_bound() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(|request: &wiremock::Request| {
            let id: u64 = request
                .url
                .path()
                .rsplit('/')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"id":id,"name":"Board"}))
        })
        .expect(131)
        .mount(&server)
        .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    for id in 1..=130 {
        discovery::board_from(&client, &config, &id.to_string())
            .await
            .unwrap();
    }
    discovery::board_from(&client, &config, "130")
        .await
        .unwrap();
    discovery::board_from(&client, &config, "1").await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 131);
}

#[tokio::test]
async fn semantically_invalid_direct_responses_are_not_cached_and_can_be_retried() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for is_project in [true, false] {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .respond_with(move |_: &wiremock::Request| {
                let first = count.fetch_add(1, Ordering::Relaxed) == 0;
                let value = if is_project {
                    json!({"key":if first {"TWO"} else {"ONE"},"name":"Project"})
                } else {
                    json!({"id":if first {8} else {7},"name":"Board"})
                };
                ResponseTemplate::new(200).set_body_json(value)
            })
            .expect(2)
            .mount(&server)
            .await;
        let config = jira(&server.uri());
        let client = JiraClient::new(&config, &Upstream::default())
            .await
            .unwrap();
        if is_project {
            assert!(
                discovery::project_from(&client, &config, "ONE")
                    .await
                    .is_err()
            );
            assert_eq!(
                discovery::project_from(&client, &config, "ONE")
                    .await
                    .unwrap()[0]
                    .id,
                "ONE"
            );
        } else {
            assert!(discovery::board_from(&client, &config, "7").await.is_err());
            assert_eq!(
                discovery::board_from(&client, &config, "7")
                    .await
                    .unwrap()
                    .id,
                "7"
            );
        }
    }
}

#[tokio::test]
async fn successful_responses_larger_than_the_cache_byte_bound_are_not_retained() {
    let server = MockServer::start().await;
    mock_json(
        &server,
        "/rest/api/3/field",
        json!([{"id":"customfield_1","name":"x".repeat(16 * 1024 * 1024)}]),
        2,
    )
    .await;
    let config = jira(&server.uri());
    let limits = Upstream {
        max_response_bytes: 32 * 1024 * 1024,
        ..Upstream::default()
    };
    let client = JiraClient::new(&config, &limits).await.unwrap();
    client.get_json("rest/api/3/field", &[]).await.unwrap();
    client.get_json("rest/api/3/field", &[]).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn failed_or_incomplete_name_searches_can_be_retried_then_success_is_reused() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for first in [
        json!({"startAt":4,"total":1,"values":[{"key":"ONE","name":"Project name"}]}),
        json!({"startAt":0,"total":2,"isLast":true,"values":[{"key":"ONE","name":"Project name"}]}),
        json!({"startAt":0,"isLast":false,"values":[{"key":"ONE","name":"Project name"}]}),
        json!({"startAt":0,"total":1,"values":[{"key":"ONE","name":"Other name"}]}),
    ] {
        let server = MockServer::start().await;
        let count = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/rest/api/3/project/search"))
            .and(query_param("query", "Project name"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(
                    if count.fetch_add(1, Ordering::Relaxed) == 0 {
                        first.clone()
                    } else {
                        json!({"startAt":0,"total":1,"values":[{"key":"ONE","name":"Project name"}]})
                    },
                )
            })
            .expect(2)
            .mount(&server)
            .await;
        let config = jira(&server.uri());
        let client = JiraClient::new(
            &config,
            &Upstream {
                max_pages: 1,
                ..Upstream::default()
            },
        )
        .await
        .unwrap();
        assert!(
            discovery::project_from(&client, &config, "Project name")
                .await
                .is_err()
        );
        for _ in 0..2 {
            assert_eq!(
                discovery::project_from(&client, &config, "Project name")
                    .await
                    .unwrap()[0]
                    .id,
                "ONE"
            );
        }
    }
}

#[tokio::test]
async fn field_inventory_missing_required_id_is_not_retained_for_metadata_retry() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/rest/api/3/field"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(
                if count.fetch_add(1, Ordering::Relaxed) == 0 {
                    json!([{"name":"AssignedTeam"}])
                } else {
                    json!([{"id":"customfield_1","name":"AssignedTeam"}])
                },
            )
        })
        .expect(2)
        .mount(&server)
        .await;
    let client = JiraClient::new(&jira(&server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert!(
        client.get_json("rest/api/3/field", &[]).await.unwrap()[0]
            .get("id")
            .is_none()
    );
    for _ in 0..2 {
        assert_eq!(
            client.get_json("rest/api/3/field", &[]).await.unwrap()[0]["id"],
            "customfield_1"
        );
    }
}

#[tokio::test]
async fn recent_board_endpoint_failure_stays_visible_and_manual_lookup_remains_usable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/greenhopper/1.0/rapidviews/recent"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"errorMessages":["Recent boards are unavailable"]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    mock_json(
        &server,
        "/rest/agile/1.0/board/7",
        json!({"id":7,"name":"Manual board"}),
        1,
    )
    .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    let error = discovery::boards_from(&client).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("recent Jira boards"));
    assert!(message.contains("HTTP 404"));
    assert!(message.contains("manual lookup remains available"));
    discovery::board_from(&client, &config, "7").await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn diagnostic_causes_are_contextual_and_do_not_expose_auth_bodies_html_or_terminal_controls()
{
    for status in [401, 403, 404, 429, 503] {
        let server = MockServer::start().await;
        let response = ResponseTemplate::new(status).insert_header("retry-after", "2").set_body_json(json!({
            "errorMessages":["Safe Jira detail", "synthetic-test-token", "synthetic%2Dtest%2Dtoken", "Basic ZmFrZUBleGFtcGxlLmludmFsaWQ6c3ludGhldGljLXRlc3QtdG9rZW4=", "<html>private raw body</html>", "\u{001b}[31mterminal\u{0007}"],
            "private":"never-display-this-body-field"
        }));
        Mock::given(method("GET"))
            .and(path("/rest/agile/1.0/board/7"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let config = jira(&server.uri());
        let client = JiraClient::new(&config, &Upstream::default())
            .await
            .unwrap();
        let error = discovery::board_from(&client, &config, "7")
            .await
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("verify submitted Jira board"));
        assert!(message.contains(&format!("HTTP {status}")), "{message}");
        assert!(message.contains("Safe Jira detail"));
        if status == 429 {
            assert!(message.contains("retry after"), "{message}");
        }
        for forbidden in [
            "synthetic-test-token",
            "synthetic%2Dtest%2Dtoken",
            "ZmFrZUBleGFtcGxlLmludmFsaWQ6c3ludGhldGljLXRlc3QtdG9rZW4=",
            "<html>",
            "private raw body",
            "never-display-this-body-field",
            "\u{001b}",
            "\u{0007}",
            "http://",
            "authorization",
        ] {
            assert!(
                !message.contains(forbidden),
                "diagnostic exposed {forbidden}"
            );
        }
    }
}

#[tokio::test]
async fn malformed_oversized_and_timed_out_reads_keep_specific_causes_and_are_not_cached() {
    for (response, expected) in [
        (
            ResponseTemplate::new(200).set_body_string("<html>private body</html>"),
            "malformed",
        ),
        (
            ResponseTemplate::new(200).set_body_string("x".repeat(2048)),
            "1024 byte limit",
        ),
        (
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(1500))
                .set_body_json(json!({"id":7,"name":"Board"})),
            "1 second request limit",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/agile/1.0/board/7"))
            .respond_with(response)
            .expect(2)
            .mount(&server)
            .await;
        let config = jira(&server.uri());
        let limits = Upstream {
            max_response_bytes: 1024,
            timeout_seconds: 1,
            ..Upstream::default()
        };
        let client = JiraClient::new(&config, &limits).await.unwrap();
        for _ in 0..2 {
            let error = discovery::board_from(&client, &config, "7")
                .await
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("verify submitted Jira board"));
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("private body"));
        }
    }
}

#[tokio::test]
async fn pagination_diagnostics_identify_read_offset_and_configured_incomplete_bound() {
    let mut fake = Fake::new(vec![(
        "rest/api/3/project/search",
        json!({"startAt":0,"values":[{"key":"ONE","name":"One"}]}),
    )]);
    fake.limits.max_pages = 1;
    let error = discovery::all_projects_from(&fake).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Jira project inventory"));
    assert!(message.contains("limit of 1 pages"));
    assert!(message.contains("incomplete"));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/project/search"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    let error = discovery::all_projects_from(&client).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Jira project inventory page at offset 0"));
    assert!(message.contains("HTTP 403"));
}

fn team_metadata() -> discovery::Metadata {
    discovery::Metadata {
        board_id: 7,
        team_field_name: "AssignedTeam".into(),
        team_field_id: Some("customfield_42".into()),
        types: vec![discovery::TypeMetadata {
            project: "ONE".into(),
            name: "Task".into(),
            teams: vec!["Alpha".into(), "Beta".into()],
            team_options: vec![
                discovery::TeamOption {
                    id: "100".into(),
                    value: "Alpha".into(),
                },
                discovery::TeamOption {
                    id: "101".into(),
                    value: "Beta".into(),
                },
            ],
            statuses: vec![],
            notes: vec![],
        }],
        sprints: vec![
            discovery::Sprint {
                id: "11".into(),
                name: "Does not match the configured regex".into(),
                state: "active".into(),
            },
            discovery::Sprint {
                id: "12".into(),
                name: "Another foreign active sprint".into(),
                state: "active".into(),
            },
            discovery::Sprint {
                id: "13".into(),
                name: "Team future".into(),
                state: "future".into(),
            },
        ],
    }
}

fn create_team_metadata_fake(options: Value) -> Fake {
    Fake::new(vec![
        (
            "rest/api/3/group/member",
            json!({"startAt":0,"total":0,"values":[]}),
        ),
        (
            "rest/api/3/field",
            json!([{"id":"customfield_42","name":"AssignedTeam"}]),
        ),
        ("rest/agile/1.0/board/7", json!({"id":7})),
        (
            "rest/agile/1.0/board/7/sprint",
            json!({"startAt":0,"isLast":true,"values":[]}),
        ),
        (
            "rest/api/3/issue/createmeta/ONE/issuetypes",
            json!({"startAt":0,"total":1,"issueTypes":[{"id":"10","name":"Task"}]}),
        ),
        (
            "rest/api/3/project/ONE/statuses",
            json!([{"id":"10","name":"Task","statuses":[]}]),
        ),
        (
            "rest/api/3/issue/createmeta/ONE/issuetypes/10",
            json!({"startAt":0,"total":1,"fields":[{"fieldId":"customfield_42","operations":["set"],"schema":{"type":"option","custom":"com.atlassian.jira.plugin.system.customfieldtypes:select"},"allowedValues":options}]}),
        ),
    ])
}

#[tokio::test]
async fn team_metadata_retains_only_enabled_unambiguous_option_pairs_and_never_infers_uuids() {
    let fake = create_team_metadata_fake(json!([
        {"id":"1","value":"Ambiguous"},
        {"id":"2","value":"Ambiguous"},
        {"id":"3","value":"Disabled","disabled":true},
        {"id":"4","value":"Verified"},
    ]));
    let metadata = discovery::metadata_from(&fake, &jira("https://example.atlassian.net"))
        .await
        .unwrap();
    assert_eq!(metadata.types[0].teams, ["Verified"]);
    assert_eq!(
        metadata.types[0].team_options,
        [discovery::TeamOption {
            id: "4".into(),
            value: "Verified".into()
        }]
    );
    let unsupported =
        create_team_metadata_fake(json!([{"id":"guessed-uuid","value":"Not a select"}]));
    unsupported.responses.lock().unwrap().back_mut().unwrap().1["fields"][0]["schema"]["type"] =
        json!("string");
    let metadata = discovery::metadata_from(&unsupported, &jira("https://example.atlassian.net"))
        .await
        .unwrap();
    assert!(metadata.types[0].teams.is_empty());
    assert!(metadata.types[0].team_options.is_empty());
}

#[tokio::test]
async fn team_create_metadata_rejects_malformed_untrusted_and_duplicate_option_identities() {
    for options in [
        json!([{"id":"1"}]),
        json!([{"id":1,"value":"Alpha"}]),
        json!([{"id":"1","value":"Alpha","disabled":"false"}]),
        json!([{"id":"1","value":"Alpha\u{001b}[31m"}]),
        json!([{"id":"1","value":"Alpha"},{"id":"1","value":"Beta"}]),
        json!({"id":"1","value":"Alpha"}),
    ] {
        let fake = create_team_metadata_fake(options);
        assert!(
            discovery::metadata_from(&fake, &jira("https://example.atlassian.net"))
                .await
                .is_err()
        );
    }
}

fn team_issue(id: &str, option: Value) -> Value {
    json!({"id":id,"fields":{"customfield_42":option}})
}

fn issue_page(issues: Vec<Value>, last: bool, token: Value) -> Value {
    json!({"issues":issues,"isLast":last,"nextPageToken":token})
}

const FIRST_SPRINT_ISSUES: &str = "rest/software/1.0/board/7/sprint/11/issue";
const SECOND_SPRINT_ISSUES: &str = "rest/software/1.0/board/7/sprint/12/issue";

#[tokio::test]
async fn board_teams_use_all_selected_board_active_sprints_and_complete_token_pagination_only() {
    let fake = Fake::new(vec![
        (
            FIRST_SPRINT_ISSUES,
            issue_page(
                vec![team_issue("1", json!({"id":"100","value":"Alpha"}))],
                false,
                json!("opaque+/=token"),
            ),
        ),
        (
            FIRST_SPRINT_ISSUES,
            issue_page(vec![team_issue("2", Value::Null)], true, Value::Null),
        ),
        (
            SECOND_SPRINT_ISSUES,
            issue_page(
                vec![team_issue("3", json!({"id":"101","value":"Beta"}))],
                true,
                Value::Null,
            ),
        ),
    ]);
    let result = discovery::board_teams_from(
        &fake,
        &jira("https://example.atlassian.net"),
        &team_metadata(),
    )
    .await
    .unwrap();
    assert_eq!(result.teams, ["Alpha", "Beta"]);
    assert_eq!(result.unverified_values, 0);
    let requests = fake.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (_, params) in requests.iter() {
        assert!(params.contains(&("fields".into(), "customfield_42".into())));
        assert!(params.contains(&("maxResults".into(), "100".into())));
        assert!(
            !params
                .iter()
                .any(|(key, _)| matches!(key.as_str(), "jql" | "startAt" | "state"))
        );
    }
    assert!(!requests[0].1.iter().any(|(key, _)| key == "nextPageToken"));
    assert!(
        requests[1]
            .1
            .contains(&("nextPageToken".into(), "opaque+/=token".into()))
    );
    assert!(!requests[2].1.iter().any(|(key, _)| key == "nextPageToken"));
}

#[tokio::test]
async fn board_teams_intersect_observed_option_id_and_value_with_selected_create_metadata() {
    let fake = Fake::new(vec![
        (
            FIRST_SPRINT_ISSUES,
            issue_page(
                vec![
                    team_issue("1", json!({"id":"100","value":"Alpha"})),
                    team_issue("2", json!({"id":"999","value":"Beta"})),
                    team_issue("3", json!({"id":"101","value":"Untrusted label"})),
                    team_issue(
                        "4",
                        json!({"id":"100","value":"Alpha","self":"https://evil.invalid/option"}),
                    ),
                ],
                true,
                Value::Null,
            ),
        ),
        (SECOND_SPRINT_ISSUES, issue_page(vec![], true, Value::Null)),
    ]);
    let result = discovery::board_teams_from(
        &fake,
        &jira("https://example.atlassian.net"),
        &team_metadata(),
    )
    .await
    .unwrap();
    assert_eq!(result.teams, ["Alpha"]);
    assert_eq!(result.unverified_values, 2);
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn completed_board_issue_pages_may_omit_the_optional_continuation_token() {
    let fake = Fake::new(vec![
        (
            FIRST_SPRINT_ISSUES,
            json!({"issues":[team_issue("1", json!({"id":"100","value":"Alpha"}))],"isLast":true}),
        ),
        (SECOND_SPRINT_ISSUES, json!({"issues":[],"isLast":true})),
    ]);
    let result = discovery::board_teams_from(
        &fake,
        &jira("https://example.atlassian.net"),
        &team_metadata(),
    )
    .await
    .unwrap();
    assert_eq!(result.teams, ["Alpha"]);
}

#[tokio::test]
async fn board_teams_fail_closed_on_malformed_untrusted_issue_values() {
    for row in [
        json!({"fields":{"customfield_42":null}}),
        json!({"id":"1","fields":{}}),
        team_issue("1", json!("Alpha")),
        team_issue("1", json!([{"id":"100","value":"Alpha"}])),
        team_issue("1", json!({"id":"100"})),
        team_issue("1", json!({"id":100,"value":"Alpha"})),
        team_issue("1", json!({"id":"100","value":"Alpha\u{001b}[31m"})),
        team_issue("1", json!({"id":"100","value":"\u{202e}Alpha"})),
    ] {
        let fake = Fake::new(vec![(
            FIRST_SPRINT_ISSUES,
            issue_page(vec![row], true, Value::Null),
        )]);
        assert!(
            discovery::board_teams_from(
                &fake,
                &jira("https://example.atlassian.net"),
                &team_metadata()
            )
            .await
            .is_err()
        );
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn board_team_token_pages_reject_missing_or_contradictory_completion_and_empty_continuation()
{
    let row = team_issue("1", json!({"id":"100","value":"Alpha"}));
    for page in [
        json!({"issues":[row.clone()],"nextPageToken":null}),
        json!({"issues":[row.clone()],"isLast":"true","nextPageToken":null}),
        json!({"issues":[row.clone()],"isLast":false}),
        json!({"issues":{},"isLast":true,"nextPageToken":null}),
        issue_page(vec![row.clone()], true, json!("extra")),
        issue_page(vec![row.clone()], false, Value::Null),
        issue_page(vec![row.clone()], false, json!(23)),
        issue_page(vec![row.clone()], false, json!("")),
        issue_page(vec![row.clone()], false, json!("\n")),
        issue_page(vec![row], false, json!("x".repeat(4097))),
        issue_page(vec![], false, json!("more")),
    ] {
        let fake = Fake::new(vec![(FIRST_SPRINT_ISSUES, page)]);
        assert!(
            discovery::board_teams_from(
                &fake,
                &jira("https://example.atlassian.net"),
                &team_metadata()
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn board_team_pagination_repeated_tokens_issue_identities_and_page_bounds_fail_without_partial_teams()
 {
    for duplicate_token in [true, false] {
        let fake = Fake::new(vec![
            (
                FIRST_SPRINT_ISSUES,
                issue_page(
                    vec![team_issue("1", json!({"id":"100","value":"Alpha"}))],
                    false,
                    json!("more"),
                ),
            ),
            (
                FIRST_SPRINT_ISSUES,
                issue_page(
                    vec![team_issue(
                        if duplicate_token { "2" } else { "1" },
                        json!({"id":"101","value":"Beta"}),
                    )],
                    false,
                    json!(if duplicate_token { "more" } else { "next" }),
                ),
            ),
        ]);
        let error = discovery::board_teams_from(
            &fake,
            &jira("https://example.atlassian.net"),
            &team_metadata(),
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains("repeated"));
    }
    let mut fake = Fake::new(vec![(
        FIRST_SPRINT_ISSUES,
        issue_page(
            vec![team_issue("1", json!({"id":"100","value":"Alpha"}))],
            false,
            json!("more"),
        ),
    )]);
    fake.limits.max_pages = 1;
    let error = discovery::board_teams_from(
        &fake,
        &jira("https://example.atlassian.net"),
        &team_metadata(),
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("limit of 1 pages"));
}

#[tokio::test]
async fn failure_in_a_later_active_sprint_discards_earlier_verified_teams() {
    let fake = Fake::new(vec![
        (
            FIRST_SPRINT_ISSUES,
            issue_page(
                vec![team_issue("1", json!({"id":"100","value":"Alpha"}))],
                true,
                Value::Null,
            ),
        ),
        (
            SECOND_SPRINT_ISSUES,
            json!({"issues":[],"isLast":false,"nextPageToken":null}),
        ),
    ]);
    assert!(
        discovery::board_teams_from(
            &fake,
            &jira("https://example.atlassian.net"),
            &team_metadata()
        )
        .await
        .is_err()
    );
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn board_team_scope_and_unsafe_field_or_sprint_ids_cannot_trigger_untrusted_paths() {
    for change in 0..6 {
        let mut metadata = team_metadata();
        match change {
            0 => metadata.board_id = 8,
            1 => metadata.team_field_name = "Other field".into(),
            2 => metadata.team_field_id = Some("customfield_42&fields=secret".into()),
            3 => metadata.team_field_id = None,
            4 => metadata.types[0].project = "OTHER".into(),
            5 => metadata.sprints[0].id = "https://evil.invalid/issue".into(),
            _ => unreachable!(),
        }
        let fake = Fake::new(vec![]);
        assert!(
            discovery::board_teams_from(&fake, &jira("https://example.atlassian.net"), &metadata)
                .await
                .is_err()
        );
        assert!(fake.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn no_active_sprints_produces_honest_empty_board_teams_without_any_scan() {
    let mut metadata = team_metadata();
    metadata.sprints.retain(|s| s.state == "future");
    let fake = Fake::new(vec![]);
    assert!(
        discovery::board_teams_from(&fake, &jira("https://example.atlassian.net"), &metadata)
            .await
            .unwrap()
            .teams
            .is_empty()
    );
    assert!(fake.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn official_board_issue_endpoint_uses_bounded_read_only_transport_and_never_follows_response_urls()
 {
    let server = MockServer::start().await;
    let route = format!("/{FIRST_SPRINT_ISSUES}");
    Mock::given(method("GET"))
        .and(path(&route))
        .and(query_param("fields", "customfield_42"))
        .and(query_param("maxResults", "100"))
        .respond_with(|request: &wiremock::Request| {
            if request
                .url
                .query_pairs()
                .any(|(key, value)| key == "nextPageToken" && value == "opaque-token")
            {
                ResponseTemplate::new(403).set_body_json(json!({"errorMessages":["not permitted"]}))
            } else {
                let mut page = issue_page(
                    vec![team_issue("1", json!({"id":"100","value":"Alpha"}))],
                    false,
                    json!("opaque-token"),
                );
                page["next"] = json!("https://evil.invalid/never-fetch");
                ResponseTemplate::new(200).set_body_json(page)
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let config = jira(&server.uri());
    let client = JiraClient::new(&config, &Upstream::default())
        .await
        .unwrap();
    let error = discovery::board_teams_from(&client, &config, &team_metadata())
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("HTTP 403"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "GET" && r.url.path() == route)
    );
}
