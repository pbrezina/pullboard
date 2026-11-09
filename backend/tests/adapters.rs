mod common;
#[path = "adapters/github_batch.rs"]
mod github_batch;
use common::*;
use pullboard::{config::Upstream, forge, transport::ReadTransport};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};

async fn get_json(server: &MockServer, p: &str, body: Value) {
    mock_get(server, p, body).await;
}

#[tokio::test]
async fn every_forge_bootstraps_real_stable_identity_without_full_refresh_or_configured_account() {
    for (kind, path, user) in [
        ("github", "/user", github_user(700, "discovered")),
        ("gitlab", "/api/v4/user", gitlab_user(701, "discovered")),
        ("codeberg", "/api/v1/user", forgejo_user(702, "discovered")),
        ("forgejo", "/api/v1/user", forgejo_user(703, "discovered")),
    ] {
        let server = MockServer::start().await;
        get_json(&server, path, user.clone()).await;
        let adapter = forge::build(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let identity = adapter.identity().await.unwrap();
        assert_eq!(
            identity.account_id,
            user["id"].as_u64().unwrap().to_string()
        );
        assert_eq!(identity.username, "discovered");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(adapter.discover().await.is_err());
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

fn graph_actor(id: u64, login: &str) -> Value {
    json!({"databaseId":id,"login":login,"url":format!("https://github.com/{login}"),"__typename":"User"})
}

fn connection(nodes: Vec<Value>) -> Value {
    json!({"totalCount":nodes.len(),"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}})
}

fn graph_comment(body: &str, day: u32, author: Value) -> Value {
    json!({"body":body,"createdAt":format!("2026-01-{day:02}T00:00:00Z"),"url":"https://github.com/acme/widget/pull/42#discussion-1","author":author})
}

fn graph_pull(owner: &str, repo: &str, number: u64) -> Value {
    json!({"id":format!("PR_{owner}_{repo}_{number}"),"number":number,"title":"Cache DEV-1","body":"Details","url":format!("https://github.com/{owner}/{repo}/pull/{number}"),"author":graph_actor(1,"me"),"assignees":connection(vec![graph_actor(1,"me"),graph_actor(3,"zeta"),graph_actor(2,"alpha")]),"reviewRequests":connection(vec![json!({"requestedReviewer":graph_actor(3,"zeta")}),json!({"requestedReviewer":graph_actor(1,"me")}),json!({"requestedReviewer":graph_actor(2,"alpha")})]),"labels":connection(vec![json!({"name":"cache"})]),"isDraft":false,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-10T00:00:00Z","lastEditedAt":null,"editor":null,"comments":connection(vec![]),"reviews":connection(vec![]),"commits":connection(vec![json!({"commit":{"committedDate":"2026-01-10T00:00:00Z","authoredDate":"2026-01-09T00:00:00Z"}})]),"reviewThreads":connection(vec![])})
}

async fn graph_batch(server: &MockServer, pulls: Vec<Value>) {
    let mut data = json!({});
    for (i, pull) in pulls.into_iter().enumerate() {
        data[format!("r{i}")] = json!({"pullRequest":pull});
    }
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullBatch("))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":data})))
        .expect(1)
        .mount(server)
        .await;
}

async fn graph_page(server: &MockServer, id: &str, field: &str, after: &str, page: Value) {
    let mut node = json!({});
    node[field] = page;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(
            json!({"variables":{"id":id,"after":after}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"node":node}})))
        .expect(1)
        .mount(server)
        .await;
}

async fn github_fixture(reviewed: bool) -> MockServer {
    let server = MockServer::start().await;
    get_json(&server, "/user", github_user(1, "me")).await;
    get_json(&server,"/search/issues",json!({"total_count":1,"incomplete_results":false,"items":[{"number":42,"repository_url":"https://api.github.com/repos/acme/widget"}]})).await;
    get_json(&server, "/repos/acme/widget/issues/42/timeline", json!([])).await;
    let mut pull = graph_pull("acme", "widget", 42);
    let mut comments = vec![graph_comment("Flat feedback", 1, graph_actor(2, "alpha"))];
    if reviewed {
        pull["reviews"] = connection(vec![
            json!({"body":"My submitted review is not a flat reply","author":graph_actor(1,"me"),"submittedAt":"2026-01-03T00:00:00Z","state":"COMMENTED"}),
        ]);
    } else {
        comments.push(graph_comment(
            "Answered flat discussion",
            3,
            graph_actor(1, "me"),
        ));
    }
    pull["comments"] = connection(comments);
    pull["reviewThreads"] = connection(vec![
        json!({"id":"thread-1","isResolved":false,"comments":connection(vec![graph_comment("Unanswered review feedback",2,graph_actor(2,"alpha"))])}),
    ]);
    pull["reviewThreads"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":"threads-1"});
    pull["reviewThreads"]["nodes"][0]["comments"]["pageInfo"] =
        json!({"hasNextPage":true,"endCursor":"comments-1"});
    graph_batch(&server, vec![pull]).await;
    let mut empty = connection(vec![]);
    empty["totalCount"] = json!(1);
    graph_page(&server, "thread-1", "comments", "comments-1", empty.clone()).await;
    graph_page(
        &server,
        "PR_acme_widget_42",
        "reviewThreads",
        "threads-1",
        empty,
    )
    .await;
    server
}
#[tokio::test]
async fn github_real_rest_and_graphql_keep_unanswered_review_thread_after_push() {
    let server = github_fixture(false).await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = pullboard::sync::Progress::recording("test-forge");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(1));
    let p = &data.pull_requests[0];
    assert_eq!(
        p.reviewers
            .iter()
            .map(|p| p.username.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert_eq!(p.assignees.len(), 2);
    assert_eq!(p.linked_issue_keys, ["DEV-1"]);
    let a = p.attention.as_ref().unwrap();
    assert_eq!(a.threads.len(), 1);
    assert_eq!(a.threads[0].id, "thread-1");
    assert!(p.last_commit_at.unwrap() > a.waiting_since);
    assert!(!data.coverage_notes.is_empty());
    assert_eq!(a.coverage, "partial");
    for request in server.received_requests().await.unwrap() {
        if request.method == "POST" {
            let v: Value = serde_json::from_slice(&request.body).unwrap();
            assert!(v["query"].as_str().unwrap().starts_with("query "));
            assert!(!v["query"].as_str().unwrap().contains("mutation"));
        } else {
            assert_eq!(request.method, "GET");
        }
    }
}
#[tokio::test]
async fn github_submitted_review_does_not_clear_unrelated_flat_discussion() {
    let server = github_fixture(true).await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let data = adapter.discover().await.unwrap();
    let attention = data.pull_requests[0].attention.as_ref().unwrap();
    assert!(attention.threads.iter().any(|t| t.id == "conversation"));
    assert!(attention.threads.iter().any(|t| t.id == "thread-1"));
}
#[tokio::test]
async fn github_search_limits_fail_without_publishing_incomplete_data() {
    let server = MockServer::start().await;
    get_json(&server, "/user", github_user(1, "me")).await;
    get_json(
        &server,
        "/search/issues",
        json!({"total_count":1001,"incomplete_results":false,"items":[]}),
    )
    .await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert_eq!(adapter.discover().await.unwrap_err().code, "search_limit");
}

#[tokio::test]
async fn every_forge_counts_only_fully_hydrated_unique_items_not_partly_fetched_prs() {
    for kind in ["github", "gitlab", "codeberg", "forgejo"] {
        let server = MockServer::start().await;
        let (root, user_path, user, candidate_path, candidates, detail) = match kind {
            "github" => (
                "/repos/acme/widget/pulls/42",
                "/user",
                github_user(1, "me"),
                "/search/issues",
                json!({"total_count":2,"incomplete_results":false,"items":[{"number":42,"repository_url":"https://api.github.com/repos/acme/widget"},{"number":42,"repository_url":"https://api.github.com/repos/acme/widget"}]}),
                json!({"number":42,"title":"Example","body":"","html_url":"https://github.com/acme/widget/pull/42","user":github_user(1,"me"),"assignees":[],"requested_reviewers":[],"labels":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"}),
            ),
            "gitlab" => (
                "/api/v4/projects/10/merge_requests/42",
                "/api/v4/user",
                gitlab_user(1, "me"),
                "/api/v4/merge_requests",
                json!([{"iid":42,"project_id":10,"title":"Example","description":"","web_url":"https://gitlab.example/acme/widget/-/merge_requests/42","author":gitlab_user(1,"me"),"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","state":"opened"}]),
                json!({}),
            ),
            _ => (
                "/api/v1/repos/acme/widget/pulls/42",
                "/api/v1/user",
                forgejo_user(1, "me"),
                "/api/v1/repos/issues/search",
                json!([{"number":42,"repository":{"full_name":"acme/widget"}},{"number":42,"repository":{"full_name":"acme/widget"}}]),
                json!({"number":42,"title":"Example","body":"","html_url":"https://forge.example/acme/widget/pulls/42","user":forgejo_user(1,"me"),"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"}),
            ),
        };
        get_json(&server, user_path, user).await;
        get_json(&server, candidate_path, candidates).await;
        if kind == "gitlab" {
            get_json(&server, "/api/v4/todos", json!([])).await;
            get_json(
                &server,
                "/api/v4/projects/10",
                json!({"path_with_namespace":"acme/widget"}),
            )
            .await;
            get_json(&server, &format!("{root}/discussions"), json!([])).await;
        } else {
            get_json(&server, root, detail).await;
            get_json(&server, &format!("{root}/reviews"), json!([])).await;
            let issue_root = if kind == "github" {
                "/repos/acme/widget/issues/42"
            } else {
                "/api/v1/repos/acme/widget/issues/42"
            };
            get_json(&server, &format!("{issue_root}/comments"), json!([])).await;
            if kind == "github" {
                get_json(&server, &format!("{issue_root}/timeline"), json!([])).await;
            }
        }
        if kind == "github" {
            let mut pull = graph_pull("acme", "widget", 42);
            pull["commits"]["totalCount"] = json!(2);
            pull["commits"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":"commit-page-1"});
            graph_batch(&server, vec![pull]).await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_partial_json(
                    json!({"variables":{"id":"PR_acme_widget_42","after":"commit-page-1"}}),
                ))
                .respond_with(ResponseTemplate::new(403))
                .expect(1)
                .mount(&server)
                .await;
        } else {
            Mock::given(method("GET"))
                .and(path(format!("{root}/commits")))
                .respond_with(ResponseTemplate::new(403))
                .expect(1)
                .mount(&server)
                .await;
        }
        let adapter = forge::build(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let progress = pullboard::sync::Progress::recording("test-forge");
        assert!(
            adapter
                .discover_with_progress(progress.clone())
                .await
                .is_err()
        );
        assert_eq!(progress.snapshot().unwrap().completed_items, 0);
        assert_eq!(progress.snapshot().unwrap().total_items, Some(1));
    }
}
#[tokio::test]
async fn gitlab_uses_account_scopes_private_project_and_paginated_discussions() {
    let server = MockServer::start().await;
    get_json(&server, "/api/v4/user", gitlab_user(1, "me")).await;
    get_json(&server,"/api/v4/merge_requests",json!([{"iid":7,"project_id":10,"title":"A private change","description":"DEV-1","web_url":"https://gitlab.example/group/private/-/merge_requests/7","author":gitlab_user(1,"me"),"assignees":[gitlab_user(1,"me")],"reviewers":[gitlab_user(1,"me"),gitlab_user(2,"alice")],"labels":["private"],"draft":true,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-04T00:00:00Z","state":"opened"}])).await;
    get_json(&server, "/api/v4/todos", json!([])).await;
    get_json(
        &server,
        "/api/v4/projects/10",
        json!({"path_with_namespace":"group/private"}),
    )
    .await;
    get_json(&server,"/api/v4/projects/10/merge_requests/7/discussions",json!([{"id":"diff","individual_note":false,"notes":[{"id":1,"body":"Please fix this","author":gitlab_user(2,"alice"),"created_at":"2026-01-02T00:00:00Z","system":false,"resolvable":true,"resolved":false}]},{"id":"flat","individual_note":true,"notes":[{"id":2,"body":"Earlier feedback","author":gitlab_user(2,"alice"),"created_at":"2026-01-01T00:00:00Z","system":false},{"id":3,"body":"My answer","author":gitlab_user(1,"me"),"created_at":"2026-01-03T00:00:00Z","system":false}]}])).await;
    get_json(
        &server,
        "/api/v4/projects/10/merge_requests/7/commits",
        json!([{"committed_date":"2026-01-04T00:00:00Z"}]),
    )
    .await;
    let adapter = forge::build(&forge("gitlab", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = pullboard::sync::Progress::recording("test-forge");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(1));
    let p = &data.pull_requests[0];
    assert_eq!(p.owner, "group");
    assert_eq!(p.repository, "private");
    assert_eq!(p.state, "draft");
    assert!(p.assignees.is_empty());
    assert_eq!(p.reviewers[0].username, "alice");
    assert_eq!(p.attention.as_ref().unwrap().threads[0].id, "diff");
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
async fn codeberg_and_forgejo_discover_account_involvement_across_repositories_read_only() {
    for kind in ["codeberg", "forgejo"] {
        let server = MockServer::start().await;
        get_json(&server, "/api/v1/user", forgejo_user(1, "me")).await;
        get_json(
            &server,
            "/api/v1/repos/issues/search",
            json!([{"number": 3, "repository": {"full_name":"acme/private"}}]),
        )
        .await;
        // Actual Forgejo responses use null for absent lists and team-only users.
        let p = json!({"number":3,"title":"A change","body":"DEV-1","html_url":"https://forgejo.example/acme/private/pulls/3","user":forgejo_user(1,"me"),"assignees":null,"requested_reviewers":null,"labels":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-03T00:00:00Z"});
        get_json(&server, "/api/v1/repos/acme/private/pulls/3", p).await;
        get_json(&server,"/api/v1/repos/acme/private/issues/3/comments",json!([{"id":1,"body":"Human feedback","user":forgejo_user(2,"alice"),"created_at":"2026-01-02T00:00:00Z","html_url":null}])).await;
        get_json(
            &server,
            "/api/v1/repos/acme/private/pulls/3/reviews",
            json!([{"id": 7, "user": null, "body": "", "state": "REQUEST_REVIEW", "submitted_at": "2026-01-02T00:00:00Z"}]),
        )
        .await;
        get_json(
            &server,
            "/api/v1/repos/acme/private/pulls/3/commits",
            json!([]),
        )
        .await;
        let adapter = forge::build(&forge(kind, &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let progress = pullboard::sync::Progress::recording("test-forge");
        let data = adapter
            .discover_with_progress(progress.clone())
            .await
            .unwrap();
        assert_eq!(progress.snapshot().unwrap().completed_items, 1);
        assert_eq!(progress.snapshot().unwrap().total_items, Some(1));
        assert_eq!(data.pull_requests.len(), 1);
        assert!(data.pull_requests[0].assignees.is_empty());
        assert!(data.pull_requests[0].attention.is_some());
        let requests = server.received_requests().await.unwrap();
        for filter in ["created", "assigned", "mentioned", "review_requested"] {
            assert!(
                requests
                    .iter()
                    .any(|r| r.url.query_pairs().any(|(k, v)| k == filter && v == "true"))
            );
        }
        // The same result found through several scopes is fetched only once.
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.path() == "/api/v1/repos/acme/private/pulls/3")
                .count(),
            1
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
async fn transport_paginates_and_reuses_etags() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([1, 2])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([3])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/conditional"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"one\"")
                .set_body_json(json!({"value":42})),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/conditional"))
        .and(header("if-none-match", "\"one\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    let limits = Upstream {
        page_size: 2,
        ..Default::default()
    };
    let t = ReadTransport::new(&forge("forgejo", &server.uri()), &limits)
        .await
        .unwrap();
    let items: Vec<i32> = t.pages("items", &[]).await.unwrap();
    assert_eq!(items, [1, 2, 3]);
    let first: Value = t.get("conditional", &[]).await.unwrap();
    let second: Value = t.get("conditional", &[]).await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn pagination_honors_next_link_when_server_clamps_page_size() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    format!("<{}/api/v1/items?page=2>; rel=\"next\"", server.uri()),
                )
                .set_body_json(json!([1])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    format!("<{}/api/v1/items?page=1>; rel=\"prev\"", server.uri()),
                )
                .set_body_json(json!([2])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let t = ReadTransport::new(&forge("forgejo", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    assert_eq!(t.pages::<i32>("items", &[]).await.unwrap(), [1, 2]);
}
#[tokio::test]
async fn rate_limit_is_not_retried_until_upstream_deadline() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .expect(1)
        .mount(&server)
        .await;
    let t = ReadTransport::new(&forge("forgejo", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    for _ in 0..2 {
        let error = t.get::<Value>("items", &[]).await.unwrap_err();
        assert_eq!(error.status, http::StatusCode::TOO_MANY_REQUESTS);
        assert!(error.retry_at.is_some());
    }
}

#[tokio::test]
async fn gitlab_first_request_retains_retry_after_instead_of_hidden_builder_probe() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .and(header("private-token", "test-secret"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "60"))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = forge::build(&forge("gitlab", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let error = adapter.discover().await.unwrap_err();
    assert_eq!(error.status, http::StatusCode::TOO_MANY_REQUESTS);
    assert!(error.retry_at.is_some());
}
#[tokio::test]
async fn page_bounds_and_repository_filters_are_enforced() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([1])))
        .expect(2)
        .mount(&server)
        .await;
    let limits = Upstream {
        page_size: 1,
        max_pages: 2,
        ..Default::default()
    };
    let mut config = forge("codeberg", &server.uri());
    config.include_repositories = vec!["acme/*".into()];
    config.exclude_repositories = vec!["acme/old-*".into()];
    assert!(config.includes("acme/new"));
    assert!(!config.includes("acme/old-app"));
    assert!(!config.includes("other/new"));
    let t = ReadTransport::new(&config, &limits).await.unwrap();
    assert_eq!(
        t.pages::<i32>("items", &[]).await.unwrap_err().code,
        "pagination_limit"
    );
}
