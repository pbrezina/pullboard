//! GitHub wire fixtures exercise real multi-item queries, not REST-only mocks.
use super::*;

#[tokio::test]
async fn prs_without_a_current_self_review_request_do_not_fetch_unused_timeline_history() {
    let server = MockServer::start().await;
    inventory(&server, &[("acme", "widget", 42)]).await;
    let mut pull = graph_pull("acme", "widget", 42);
    pull["reviewRequests"] = connection(vec![json!({"requestedReviewer":graph_actor(2,"alpha")})]);
    graph_batch(&server, vec![pull]).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/issues/42/timeline"))
        .respond_with(ResponseTemplate::new(403))
        .expect(0)
        .with_priority(1)
        .mount(&server)
        .await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = Progress::recording("test-forge");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(data.pull_requests.len(), 1);
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert!(data.pull_requests[0].attention.is_none());
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| !r.url.path().ends_with("/timeline"))
    );
}
use pullboard::sync::Progress;
use std::time::Duration;

async fn inventory(server: &MockServer, keys: &[(&str, &str, u64)]) {
    get_json(server, "/user", github_user(1, "me")).await;
    let items: Vec<_> = keys.iter().map(|(owner, repo, number)| json!({
        "number":number,"repository_url":format!("https://api.github.com/repos/{owner}/{repo}")
    })).collect();
    get_json(
        server,
        "/search/issues",
        json!({"total_count":items.len(),"incomplete_results":false,"items":items}),
    )
    .await;
    for (owner, repo, number) in keys {
        get_json(
            server,
            &format!("/repos/{owner}/{repo}/issues/{number}/timeline"),
            json!([]),
        )
        .await;
    }
}

fn paged(nodes: Vec<Value>, total: usize, cursor: &str) -> Value {
    json!({"nodes":nodes,"totalCount":total,"pageInfo":{"hasNextPage":true,"endCursor":cursor}})
}

fn final_page(nodes: Vec<Value>, total: usize) -> Value {
    let mut page = connection(nodes);
    page["totalCount"] = json!(total);
    page
}

#[tokio::test]
async fn mixed_repositories_share_one_actual_batch_and_keep_complete_public_payloads() {
    let server = MockServer::start().await;
    inventory(
        &server,
        &[
            ("acme", "widget", 42),
            ("other", "private", 7),
            ("acme", "widget", 42),
        ],
    )
    .await;
    graph_batch(
        &server,
        vec![
            graph_pull("acme", "widget", 42),
            graph_pull("other", "private", 7),
        ],
    )
    .await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = Progress::recording("test-forge");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 2);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(2));
    assert_eq!(data.pull_requests.len(), 2);
    for (pull, (owner, repo, number)) in data
        .pull_requests
        .iter()
        .zip([("acme", "widget", 42), ("other", "private", 7)])
    {
        let person = |id, name: &str| json!({"id":format!("{id}"),"username":name,"display_name":name,"url":format!("https://github.com/{name}")});
        // This is the previous REST+GraphQL projection, including numeric user
        // IDs, stable source/repository PR IDs and null/empty fields.
        assert_eq!(
            serde_json::to_value(pull).unwrap(),
            json!({
                "id":format!("test-forge:{owner}/{repo}:{number}"),"source_id":"test-forge","forge":"github","number":number,
                "owner":owner,"repository":repo,"title":"Cache DEV-1","url":format!("https://github.com/{owner}/{repo}/pull/{number}"),
                "description":"Details","state":"ready","author":person(1,"me"),"assignees":[person(2,"alpha"),person(3,"zeta")],
                "reviewers":[person(2,"alpha"),person(3,"zeta")],"labels":["cache"],"created_at":"2026-01-01T00:00:00Z",
                "updated_at":"2026-01-10T00:00:00Z","last_commit_at":"2026-01-10T00:00:00Z","linked_issue_keys":["DEV-1"],"is_mine":true,"attention":null
            })
        );
    }
    let requests = server.received_requests().await.unwrap();
    let queries: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(queries.len(), 1);
    let body: Value = serde_json::from_slice(&queries[0].body).unwrap();
    assert_eq!(body["variables"]["repo0"], "widget");
    assert_eq!(body["variables"]["repo1"], "private");
    let query = body["query"].as_str().unwrap();
    assert!(query.contains("r0:repository") && query.contains("r1:repository"));
    for field in [
        "comments(first:",
        "reviews(first:",
        "commits(first:",
        "reviewThreads(first:",
        "assignees(first:",
        "labels(first:",
        "reviewRequests(first:",
    ] {
        assert!(query.contains(field), "{field}");
    }
    assert!(
        !requests
            .iter()
            .any(|r| r.method == "GET" && r.url.path().contains("/pulls/"))
    );
    assert_eq!(requests.len(), 8); // identity, three searches, batch, two timelines, assigned issues
}

#[tokio::test]
async fn all_connections_and_nested_review_comments_are_fully_paginated() {
    let server = MockServer::start().await;
    inventory(&server, &[("acme", "widget", 42)]).await;
    let mut pull = graph_pull("acme", "widget", 42);
    let id = "PR_acme_widget_42";
    let mut other = graph_actor(8, "beta");
    other["url"] = json!("javascript:alert(1)");
    let own = graph_actor(1, "me");
    let review = json!({"id":"review-beta","body":"Submitted feedback","author":other.clone(),"submittedAt":"2026-01-06T00:00:00Z","state":"APPROVED"});
    let thread = json!({"id":"thread-later","isResolved":false,"comments":paged(vec![graph_comment("Please fix",2,graph_actor(2,"alpha"))],2,"thread-comment-page")});
    let page_pairs = [
        ("assignees", vec![own.clone()], vec![other.clone()]),
        (
            "reviewRequests",
            vec![json!({"requestedReviewer":own.clone()})],
            vec![json!({"requestedReviewer":{"__typename":"Team"}})],
        ),
        (
            "labels",
            vec![json!({"name":"cache"})],
            vec![json!({"name":"late-label"})],
        ),
        (
            "comments",
            vec![graph_comment("Flat feedback", 1, graph_actor(2, "alpha"))],
            vec![graph_comment("Answered", 7, own.clone())],
        ),
        ("reviews", vec![], vec![review]),
        // The largest date occurs before the final commit, and author dates do
        // not replace present committer dates.
        (
            "commits",
            vec![
                json!({"commit":{"committedDate":"2026-01-20T00:00:00Z","authoredDate":"2026-01-25T00:00:00Z"}}),
            ],
            vec![
                json!({"commit":{"committedDate":"2026-01-10T00:00:00Z","authoredDate":"2026-01-09T00:00:00Z"}}),
            ],
        ),
        ("reviewThreads", vec![], vec![thread]),
    ];
    for (field, first, last) in page_pairs {
        let total = first.len() + last.len();
        let cursor = format!("{field}-page");
        pull[field] = paged(first, total, &cursor);
        graph_page(&server, id, field, &cursor, final_page(last, total)).await;
    }
    graph_page(
        &server,
        "thread-later",
        "comments",
        "thread-comment-page",
        final_page(vec![graph_comment("Answer in thread", 5, own)], 2),
    )
    .await;
    graph_batch(&server, vec![pull]).await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let data = adapter.discover().await.unwrap();
    let p = &data.pull_requests[0];
    assert_eq!(p.labels, ["cache", "late-label"]);
    assert_eq!(
        p.last_commit_at.unwrap().to_rfc3339(),
        "2026-01-20T00:00:00+00:00"
    );
    assert_eq!(p.assignees[0].username, "beta");
    assert_eq!(p.assignees[0].id, "8");
    assert_eq!(p.assignees[0].url, None);
    assert_eq!(p.reviewers[0].username, "beta");
    assert!(p.attention.is_none()); // Both flat and review threads answered.
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        9
    );
}

#[tokio::test]
async fn missing_or_error_batch_member_fails_the_whole_source_without_false_progress() {
    for body in [
        json!({"data":{"r0":{"pullRequest":graph_pull("acme","widget",42)},"r1":{"pullRequest":null}}}),
        json!({"data":{"r0":{"pullRequest":graph_pull("acme","widget",42)}}}),
        json!({"data":{"r0":{"pullRequest":graph_pull("acme","widget",42)},"r1":{"pullRequest":graph_pull("other","private",7)}},"errors":[{"type":"FORBIDDEN","path":["r1","pullRequest"]}]}),
    ] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42), ("other", "private", 7)]).await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let progress = Progress::recording("test-forge");
        assert!(
            adapter
                .discover_with_progress(progress.clone())
                .await
                .is_err()
        );
        assert_eq!(progress.snapshot().unwrap().completed_items, 0);
        assert_eq!(progress.snapshot().unwrap().total_items, Some(2));
    }
}

#[tokio::test]
async fn only_fully_evaluated_unique_prs_advance_progress_when_a_sibling_page_fails() {
    let server = MockServer::start().await;
    inventory(
        &server,
        &[
            ("acme", "widget", 42),
            ("other", "private", 7),
            ("acme", "widget", 42),
        ],
    )
    .await;
    let mut second = graph_pull("other", "private", 7);
    second["labels"] = paged(vec![], 1, "bad-labels");
    graph_batch(&server, vec![graph_pull("acme", "widget", 42), second]).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_partial_json(
            json!({"variables":{"after":"bad-labels"}}),
        ))
        .respond_with(ResponseTemplate::new(403).set_delay(Duration::from_millis(100)))
        .expect(1)
        .mount(&server)
        .await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let progress = Progress::recording("test-forge");
    assert!(
        adapter
            .discover_with_progress(progress.clone())
            .await
            .is_err()
    );
    assert_eq!(progress.snapshot().unwrap().completed_items, 1);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(2));
}

#[tokio::test]
async fn oversized_batches_split_but_auth_rate_and_read_failures_are_not_retried_as_size_errors() {
    for (status, errors, splits) in [
        (200, json!([{"type":"MAX_NODE_LIMIT_EXCEEDED"}]), true),
        (
            200,
            json!([{"extensions":{"code":"RESOURCE_LIMITS_EXCEEDED"}}]),
            true,
        ),
        (200, json!([{"type":"FORBIDDEN"}]), false),
        (
            200,
            json!([{"type":"MAX_NODE_LIMIT_EXCEEDED"},{"type":"FORBIDDEN"}]),
            false,
        ),
        (200, json!([{"type":"RATE_LIMITED"}]), false),
        (401, json!([]), false),
        (403, json!([]), false),
        (429, json!([]), false),
        (502, json!([]), false),
    ] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42), ("other", "private", 7)]).await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_partial_json(
                json!({"variables":{"number0":42,"number1":7}}),
            ))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"errors":errors})))
            .expect(1)
            .mount(&server)
            .await;
        if splits {
            for (owner, repo, number) in [("acme", "widget", 42), ("other", "private", 7)] {
                Mock::given(method("POST"))
                    .and(path("/graphql"))
                    .and(body_partial_json(
                        json!({"variables":{"owner0":owner,"repo0":repo,"number0":number}}),
                    ))
                    .and(body_string_contains(
                        "query PullBatch($first:Int!,$owner0:String!,$repo0:String!,$number0:Int!)",
                    ))
                    .respond_with(ResponseTemplate::new(200).set_body_json(
                        json!({"data":{"r0":{"pullRequest":graph_pull(owner,repo,number)}}}),
                    ))
                    .expect(1)
                    .mount(&server)
                    .await;
            }
        }
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let result = adapter.discover().await;
        assert_eq!(result.is_ok(), splits, "status {status}");
        assert_eq!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.method == "POST")
                .count(),
            if splits { 3 } else { 1 }
        );
    }
}

#[tokio::test]
async fn body_revision_and_submitted_review_timestamps_remain_separate_from_flat_replies() {
    for (editor, body_day, own_review, expected) in [
        (
            graph_actor(2, "alpha"),
            8,
            false,
            vec![("conversation", 8), ("review-request", 2)],
        ),
        (graph_actor(2, "alpha"), 1, true, vec![("conversation", 1)]),
        (graph_actor(1, "me"), 8, true, vec![]),
        (
            {
                let mut bot = graph_actor(9, "robot");
                bot["__typename"] = json!("Bot");
                bot
            },
            8,
            true,
            vec![],
        ),
    ] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        pull["body"] = json!("Please inspect @me");
        pull["lastEditedAt"] = json!(format!("2026-01-{body_day:02}T00:00:00Z"));
        pull["editor"] = editor;
        if own_review {
            pull["reviews"] = connection(vec![
                json!({"id":"own-review","body":"Review submitted","author":graph_actor(1,"me"),"state":"COMMENTED","submittedAt":"2026-01-05T00:00:00Z"}),
            ]);
        }
        Mock::given(method("GET")).and(path("/repos/acme/widget/issues/42/timeline"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"event":"review_requested","created_at":"2026-01-02T00:00:00Z","requested_reviewer":github_user(1,"me"),"actor":github_user(2,"alpha")}]))).with_priority(1).mount(&server).await;
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let data = adapter.discover().await.unwrap();
        let threads = data.pull_requests[0]
            .attention
            .as_ref()
            .map(|a| a.threads.as_slice())
            .unwrap_or_default();
        assert_eq!(threads.len(), expected.len());
        for (id, day) in expected {
            assert!(threads.iter().any(|t| t.id == id
                && t.waiting_since.to_rfc3339() == format!("2026-01-{day:02}T00:00:00+00:00")));
        }
    }
}

#[tokio::test]
async fn pagination_cycles_missing_cursors_counts_and_page_bound_fail_closed() {
    for mode in ["cycle", "missing", "count", "bound", "null-node"] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        pull["labels"] = paged(vec![], 1, "cursor");
        if mode == "missing" {
            pull["labels"]["pageInfo"]["endCursor"] = Value::Null;
        }
        if !matches!(mode, "missing" | "bound") {
            let page = match mode {
                "cycle" => paged(vec![json!({"name":"cycle"})], 1, "cursor"),
                "null-node" => final_page(vec![Value::Null], 1),
                _ => final_page(vec![], 1),
            };
            graph_page(&server, "PR_acme_widget_42", "labels", "cursor", page).await;
        }
        graph_batch(&server, vec![pull]).await;
        let limits = Upstream {
            max_pages: if mode == "bound" { 1 } else { 3 },
            ..Default::default()
        };
        let adapter = forge::build(&forge("github", &server.uri()), &limits)
            .await
            .unwrap();
        let progress = Progress::recording("test-forge");
        assert!(
            adapter
                .discover_with_progress(progress.clone())
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(progress.snapshot().unwrap().completed_items, 0);
    }
}

#[tokio::test]
async fn batch_and_item_hydration_are_concurrent_but_never_multiply_item_limits() {
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::State,
        http::Request,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Counts {
        batches: AtomicUsize,
        peak_batches: AtomicUsize,
        items: AtomicUsize,
        peak_items: AtomicUsize,
        widths: Mutex<Vec<usize>>,
    }
    async fn response(State(counts): State<Arc<Counts>>, request: Request<Body>) -> Json<Value> {
        let path = request.uri().path().to_owned();
        let value = if path == "/user" {
            github_user(1, "me")
        } else if path == "/issues" {
            json!([])
        } else if path == "/search/issues" {
            let items: Vec<_> = (1..=6).map(|number|json!({"number":number,"repository_url":"https://api.github.com/repos/acme/widget"})).collect();
            json!({"total_count":items.len(),"incomplete_results":false,"items":items})
        } else if path == "/graphql" {
            let body: Value =
                serde_json::from_slice(&to_bytes(request.into_body(), 1_000_000).await.unwrap())
                    .unwrap();
            let active = counts.batches.fetch_add(1, Ordering::SeqCst) + 1;
            counts.peak_batches.fetch_max(active, Ordering::SeqCst);
            let mut data = json!({});
            let mut width = 0;
            while let Some(number) = body["variables"][format!("number{width}")].as_u64() {
                data[format!("r{width}")] =
                    json!({"pullRequest":graph_pull("acme","widget",number)});
                width += 1;
            }
            counts.widths.lock().unwrap().push(width);
            tokio::time::sleep(Duration::from_millis(30)).await;
            counts.batches.fetch_sub(1, Ordering::SeqCst);
            json!({"data":data})
        } else {
            assert!(
                path.ends_with("/timeline"),
                "unexpected REST hydration {path}"
            );
            let active = counts.items.fetch_add(1, Ordering::SeqCst) + 1;
            counts.peak_items.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            counts.items.fetch_sub(1, Ordering::SeqCst);
            json!([])
        };
        Json(value)
    }

    for (items, batch_size, expected_batches) in [(4, 2, 2), (1, 5, 1), (3, 2, 1)] {
        let counts = Arc::new(Counts::default());
        let app = Router::new().fallback(response).with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let limits = Upstream {
            max_concurrent_requests: 16,
            max_concurrent_items: items,
            github_batch_size: batch_size,
            ..Default::default()
        };
        let adapter = forge::build(&forge("github", &format!("http://{address}")), &limits)
            .await
            .unwrap();
        let progress = Progress::recording("test-forge");
        let data = adapter
            .discover_with_progress(progress.clone())
            .await
            .unwrap();
        assert_eq!(data.pull_requests.len(), 6);
        assert_eq!(progress.snapshot().unwrap().completed_items, 6);
        assert_eq!(progress.snapshot().unwrap().total_items, Some(6));
        assert_eq!(counts.peak_batches.load(Ordering::SeqCst), expected_batches);
        assert!(counts.peak_items.load(Ordering::SeqCst) <= items);
        assert_eq!(
            counts.peak_items.load(Ordering::SeqCst),
            if items == 4 { 4 } else { items.min(batch_size) }
        );
        assert!(
            counts
                .widths
                .lock()
                .unwrap()
                .iter()
                .all(|width| *width == items.min(batch_size))
        );
        assert!(
            data.pull_requests
                .windows(2)
                .all(|pair| pair[0].number < pair[1].number)
        );
        server.abort();
    }
}

#[tokio::test]
async fn bot_events_resolved_threads_and_pending_reviews_do_not_invent_attention_or_reviewers() {
    let server = MockServer::start().await;
    inventory(&server, &[("acme", "widget", 42)]).await;
    let mut pull = graph_pull("acme", "widget", 42);
    let mut bot = graph_actor(9, "app");
    bot["__typename"] = json!("Bot");
    pull["comments"] = connection(vec![graph_comment("@me bot feedback", 9, bot.clone())]);
    pull["reviews"] = connection(vec![
        json!({"id":"bot-review","body":"@me review from bot","author":bot.clone(),"state":"COMMENTED","submittedAt":"2026-01-08T00:00:00Z"}),
        json!({"id":"pending-review","body":"Pending review","author":graph_actor(10,"pending-user"),"state":"PENDING","submittedAt":null}),
    ]);
    let mut bot_thread = graph_comment("@me bot thread", 8, bot);
    bot_thread["url"] = json!("javascript:alert(1)");
    pull["reviewThreads"] = connection(vec![
        json!({"id":"bot-thread","isResolved":false,"comments":connection(vec![bot_thread])}),
        json!({"id":"resolved-thread","isResolved":true,"comments":connection(vec![graph_comment("Human feedback",9,graph_actor(2,"alpha"))])}),
    ]);
    graph_batch(&server, vec![pull]).await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let data = adapter.discover().await.unwrap();
    assert!(data.pull_requests[0].attention.is_none());
    assert!(
        !data.pull_requests[0]
            .reviewers
            .iter()
            .any(|r| r.username == "pending-user")
    );
    assert!(
        data.pull_requests[0]
            .reviewers
            .iter()
            .any(|r| r.username == "app[bot]")
    ); // Submitted bot reviewers remain visible.
}

#[tokio::test]
async fn unsafe_pull_urls_fail_and_unsafe_thread_urls_use_the_sanitized_pull_url() {
    for unsafe_pull in [false, true] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        if unsafe_pull {
            pull["url"] = json!("javascript:alert(1)");
        }
        let mut comment = graph_comment("Please fix", 2, graph_actor(2, "alpha"));
        comment["url"] = json!("javascript:alert(1)");
        pull["reviewThreads"] = connection(vec![
            json!({"id":"thread","isResolved":false,"comments":connection(vec![comment])}),
        ]);
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let result = adapter.discover().await;
        if unsafe_pull {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap().pull_requests[0]
                    .attention
                    .as_ref()
                    .unwrap()
                    .threads[0]
                    .url,
                "https://github.com/acme/widget/pull/42"
            );
        }
    }
}

#[tokio::test]
async fn short_search_pages_do_not_truncate_inventory_and_timeline_follows_every_page() {
    let server = MockServer::start().await;
    get_json(&server, "/user", github_user(1, "me")).await;
    for page in 1..=3 {
        Mock::given(method("GET")).and(path("/search/issues")).and(query_param("page",page.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total_count":3,"incomplete_results":false,"items":[{"number":page,"repository_url":"https://api.github.com/repos/acme/widget"}]})))
            .expect(3).mount(&server).await;
    }
    graph_batch(
        &server,
        (1..=3)
            .map(|number| graph_pull("acme", "widget", number))
            .collect(),
    )
    .await;
    for number in 1..=3 {
        let path_ = format!("/repos/acme/widget/issues/{number}/timeline");
        Mock::given(method("GET")).and(path(&path_)).and(query_param("page","1"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("link",format!("<{}/{}?page=2>; rel=\"next\"",server.uri(),path_.trim_start_matches('/')))
                .set_body_json(json!([{"event":"review_requested","created_at":"2026-01-02T00:00:00Z","requested_reviewer":github_user(1,"me"),"actor":github_user(2,"alpha")}]))).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path(path_)).and(query_param("page","2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"event":"review_requested","created_at":"2026-01-08T00:00:00Z","requested_reviewer":github_user(1,"me"),"actor":github_user(2,"alpha")}]))).expect(1).mount(&server).await;
    }
    let limits = Upstream {
        page_size: 10,
        ..Default::default()
    };
    let adapter = forge::build(&forge("github", &server.uri()), &limits)
        .await
        .unwrap();
    let progress = Progress::recording("test-forge");
    let data = adapter
        .discover_with_progress(progress.clone())
        .await
        .unwrap();
    assert_eq!(progress.snapshot().unwrap().completed_items, 3);
    assert_eq!(progress.snapshot().unwrap().total_items, Some(3));
    for pull in data.pull_requests {
        let attention = pull.attention.unwrap();
        assert_eq!(attention.threads[0].id, "review-request");
        assert_eq!(
            attention.waiting_since.to_rfc3339(),
            "2026-01-08T00:00:00+00:00"
        );
    }
}

fn review_comment(body: &str, day: u32, author: Value, review_id: &str) -> Value {
    let mut comment = graph_comment(body, day, author);
    comment["pullRequestReview"] = json!({"id":review_id});
    comment
}

fn review_summary(id: &str, body: &str, day: u32) -> Value {
    json!({"id":id,"body":body,"author":graph_actor(2,"alpha"),"state":"CHANGES_REQUESTED","submittedAt":format!("2026-01-{day:02}T00:00:00Z")})
}

#[tokio::test]
async fn review_summary_clears_only_when_every_associated_thread_is_answered_or_resolved() {
    for mode in [
        "unanswered",
        "one-answer",
        "all-answered",
        "resolved",
        "mixed",
        "equal-time",
        "bot-reply",
        "reopened",
    ] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        pull["reviews"] = connection(vec![review_summary(
            "review",
            "Please see inline comments",
            3,
        )]);
        let mut threads = vec![];
        for index in 0..2 {
            let mut comments = vec![review_comment(
                "Please fix",
                2,
                graph_actor(2, "alpha"),
                "review",
            )];
            let answered = matches!(
                mode,
                "all-answered" | "equal-time" | "bot-reply" | "reopened"
            ) || (matches!(mode, "one-answer" | "mixed") && index == 0);
            if answered {
                let mut actor = graph_actor(1, "ME");
                if mode == "bot-reply" {
                    actor["__typename"] = json!("Bot");
                }
                comments.push(review_comment(
                    "Answered",
                    if mode == "equal-time" { 2 } else { 4 },
                    actor,
                    "own-review",
                ));
            }
            if mode == "reopened" && index == 1 {
                comments.push(review_comment(
                    "One more thing",
                    5,
                    graph_actor(2, "alpha"),
                    "follow-up-review",
                ));
            }
            threads.push(json!({"id":format!("thread-{index}"),"isResolved":mode == "resolved" || (mode == "mixed" && index == 1),"comments":connection(comments)}));
        }
        pull["reviewThreads"] = connection(threads);
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let data = adapter.discover().await.unwrap();
        let attention = &data.pull_requests[0].attention;
        if matches!(mode, "all-answered" | "resolved" | "mixed") {
            assert!(attention.is_none(), "{mode}: {attention:?}");
        } else {
            let attention = attention.as_ref().unwrap();
            assert!(
                attention
                    .threads
                    .iter()
                    .any(|t| t.id == "conversation" && t.excerpt == "Please see inline comments"),
                "{mode}"
            );
            assert!(
                attention
                    .threads
                    .iter()
                    .any(|t| t.id.starts_with("thread-")),
                "{mode}"
            );
        }
    }
}

#[tokio::test]
async fn handled_review_summary_does_not_clear_unrelated_flat_feedback_or_other_reviews() {
    for mode in ["flat", "standalone-review", "other-review", "description"] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        let mut reviews = vec![review_summary("handled-review", "Handled summary", 2)];
        let mut threads = vec![
            json!({"id":"handled-thread","isResolved":false,"comments":connection(vec![
                review_comment("Please fix", 2, graph_actor(2,"alpha"), "handled-review"),
                review_comment("Answered", 4, graph_actor(1,"me"), "own-review"),
            ])}),
        ];
        match mode {
            "flat" => {
                pull["comments"] = connection(vec![graph_comment(
                    "Unrelated feedback",
                    3,
                    graph_actor(2, "alpha"),
                )])
            }
            "description" => {
                pull["body"] = json!("Unrelated feedback @me");
                pull["lastEditedAt"] = json!("2026-01-03T00:00:00Z");
                pull["editor"] = graph_actor(2, "alpha");
            }
            _ => {
                reviews.push(review_summary("other-review", "Unrelated feedback", 3));
                if mode == "other-review" {
                    threads.push(json!({"id":"unanswered-thread","isResolved":false,"comments":connection(vec![
                        review_comment("Unanswered feedback", 3, graph_actor(2,"alpha"), "other-review"),
                    ])}));
                }
            }
        }
        pull["reviews"] = connection(reviews);
        pull["reviewThreads"] = connection(threads);
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let data = adapter.discover().await.unwrap();
        let attention = data.pull_requests[0].attention.as_ref().unwrap();
        assert_eq!(
            attention.waiting_since.to_rfc3339(),
            "2026-01-03T00:00:00+00:00",
            "{mode}"
        );
        assert_eq!(
            attention.threads.len(),
            if mode == "other-review" { 2 } else { 1 },
            "{mode}"
        );
        let flat = attention
            .threads
            .iter()
            .find(|t| t.id == "conversation")
            .unwrap();
        assert!(flat.excerpt.starts_with("Unrelated feedback"), "{mode}");
    }
}

#[tokio::test]
async fn standalone_review_summary_requires_a_flat_reply_not_an_unrelated_submitted_review() {
    for mode in ["unanswered", "own-review", "flat-reply"] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        let mut reviews = vec![review_summary("standalone", "Please add tests", 2)];
        if mode == "own-review" {
            let mut own = review_summary("own-review", "Unrelated review", 4);
            own["author"] = graph_actor(1, "me");
            reviews.push(own);
        }
        if mode == "flat-reply" {
            pull["comments"] =
                connection(vec![graph_comment("Tests added", 4, graph_actor(1, "me"))]);
        }
        pull["reviews"] = connection(reviews);
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let data = adapter.discover().await.unwrap();
        assert_eq!(
            data.pull_requests[0].attention.is_none(),
            mode == "flat-reply",
            "{mode}"
        );
    }
}

#[tokio::test]
async fn review_summary_clearing_uses_all_review_thread_and_reply_pages() {
    let server = MockServer::start().await;
    inventory(&server, &[("acme", "widget", 42)]).await;
    let mut pull = graph_pull("acme", "widget", 42);
    pull["reviews"] = paged(vec![], 1, "reviews-page");
    graph_page(
        &server,
        "PR_acme_widget_42",
        "reviews",
        "reviews-page",
        final_page(
            vec![review_summary("review", "Please see inline comments", 3)],
            1,
        ),
    )
    .await;
    pull["reviewThreads"] = paged(vec![], 1, "threads-page");
    graph_page(&server, "PR_acme_widget_42", "reviewThreads", "threads-page", final_page(vec![json!({
        "id":"thread","isResolved":false,"comments":paged(vec![review_comment("Please fix", 2, graph_actor(2,"alpha"), "review")], 2, "replies-page")
    })], 1)).await;
    graph_page(
        &server,
        "thread",
        "comments",
        "replies-page",
        final_page(
            vec![review_comment(
                "Answered",
                4,
                graph_actor(1, "me"),
                "own-review",
            )],
            2,
        ),
    )
    .await;
    graph_batch(&server, vec![pull]).await;
    let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
        .await
        .unwrap();
    let data = adapter.discover().await.unwrap();
    assert!(data.pull_requests[0].attention.is_none());
    for request in server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method == "POST")
    {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let query = body["query"].as_str().unwrap();
        assert!(!query.contains("mutation"));
        if body["variables"]["after"] != "reviews-page" {
            assert!(query.contains("pullRequestReview { id }"), "{query}");
        } else {
            assert!(query.contains("id body submittedAt"), "{query}");
        }
    }
}

#[tokio::test]
async fn review_summary_requires_proven_associations_and_answers_even_on_someone_elses_pr() {
    for mode in [
        "unanswered-unmentioned-thread",
        "unavailable-association",
        "answered",
        "later-bot",
    ] {
        let server = MockServer::start().await;
        inventory(&server, &[("acme", "widget", 42)]).await;
        let mut pull = graph_pull("acme", "widget", 42);
        pull["author"] = graph_actor(3, "zeta");
        pull["reviews"] = connection(vec![review_summary("review", "Please look @me", 3)]);
        let mut first = review_comment("Please fix", 2, graph_actor(2, "alpha"), "review");
        if mode == "unavailable-association" {
            first["pullRequestReview"] = Value::Null;
        }
        let mut comments = vec![first];
        if mode != "unanswered-unmentioned-thread" {
            comments.push(review_comment(
                "Answered",
                4,
                graph_actor(1, "me"),
                "own-review",
            ));
        }
        if mode == "later-bot" {
            let mut bot = graph_actor(9, "robot");
            bot["__typename"] = json!("Bot");
            comments.push(review_comment("@me automated note", 5, bot, "bot-review"));
        }
        pull["reviewThreads"] = connection(vec![
            json!({"id":"thread","isResolved":false,"comments":connection(comments)}),
        ]);
        graph_batch(&server, vec![pull]).await;
        let adapter = forge::build(&forge("github", &server.uri()), &Upstream::default())
            .await
            .unwrap();
        let data = adapter.discover().await.unwrap();
        assert_eq!(
            data.pull_requests[0].attention.is_none(),
            matches!(mode, "answered" | "later-bot"),
            "{mode}"
        );
    }
}
