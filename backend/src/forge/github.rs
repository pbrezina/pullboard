use super::{Forge, people, references};
use crate::{
    attention::{self, Event, Thread},
    cache::SourceData,
    config::{ForgeConfig, safe_url},
    error::{Error, Result},
    model::*,
    sync::Progress,
    transport::ReadTransport,
};
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct Github {
    config: ForgeConfig,
    http: Arc<ReadTransport>,
}
impl Github {
    pub fn new(config: ForgeConfig, http: Arc<ReadTransport>) -> Self {
        Self { config, http }
    }
}
#[derive(Debug, Clone, Deserialize)]
struct User {
    id: u64,
    login: String,
    #[serde(default)]
    name: Option<String>,
    html_url: Option<String>,
    #[serde(rename = "type", default)]
    kind: String,
}
impl User {
    fn person(&self) -> Person {
        Person {
            id: self.id.to_string(),
            username: self.login.clone(),
            display_name: self.name.clone().unwrap_or_else(|| self.login.clone()),
            url: self.html_url.as_deref().and_then(safe_url),
        }
    }
    fn bot(&self) -> bool {
        self.kind == "Bot" || self.login.ends_with("[bot]")
    }
}
#[derive(Deserialize)]
struct Search {
    total_count: u64,
    incomplete_results: bool,
    items: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    number: u64,
    repository_url: String,
}
struct Pull {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    user: User,
    assignees: Vec<User>,
    requested_reviewers: Vec<User>,
    labels: Vec<Label>,
    draft: Option<bool>,
    created_at: Time,
    updated_at: Time,
}
#[derive(Deserialize)]
struct Label {
    name: String,
}
struct Comment {
    body: Option<String>,
    user: User,
    created_at: Time,
}
struct Review {
    id: String,
    body: Option<String>,
    user: User,
    submitted_at: Option<Time>,
    state: String,
}
#[derive(Deserialize)]
struct Timeline {
    event: String,
    created_at: Option<Time>,
    requested_reviewer: Option<User>,
    actor: Option<User>,
}
#[derive(Deserialize)]
struct Graph<T> {
    data: Option<T>,
    errors: Option<Vec<serde_json::Value>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphPull {
    id: String,
    number: u64,
    title: String,
    body: String,
    url: String,
    author: Actor,
    is_draft: bool,
    created_at: Time,
    updated_at: Time,
    last_edited_at: Option<Time>,
    editor: Option<Actor>,
    assignees: Connection<Actor>,
    review_requests: Connection<ReviewRequest>,
    labels: Connection<Label>,
    comments: Connection<GraphComment>,
    reviews: Connection<GraphReview>,
    commits: Connection<GraphCommit>,
    review_threads: Connection<ReviewThread>,
}
#[derive(Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
    #[serde(rename = "totalCount")]
    total_count: u64,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}
#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}
#[derive(Deserialize)]
struct ReviewThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    comments: Connection<GraphComment>,
}
#[derive(Deserialize)]
struct GraphComment {
    body: String,
    #[serde(rename = "createdAt")]
    created_at: Time,
    url: String,
    author: Option<Actor>,
    #[serde(rename = "pullRequestReview")]
    review: Option<ReviewId>,
}
#[derive(Deserialize)]
struct ReviewId {
    id: String,
}
#[derive(Deserialize)]
struct Actor {
    login: String,
    #[serde(rename = "__typename")]
    kind: String,
    #[serde(rename = "databaseId")]
    database_id: Option<u64>,
    url: Option<String>,
}
impl Actor {
    fn user(&self) -> Result<User> {
        // GraphQL Bot.login omits the suffix used by REST's embedded bot users.
        // Keep the existing public username/display projection across transports.
        let login = if self.kind == "Bot" && !self.login.ends_with("[bot]") {
            format!("{}[bot]", self.login)
        } else {
            self.login.clone()
        };
        Ok(User {
            id: self.database_id.ok_or_else(Error::upstream)?,
            login,
            // Embedded REST users do not carry profile names. Do not change
            // display semantics by substituting a separate profile lookup.
            name: None,
            html_url: self.url.clone(),
            kind: self.kind.clone(),
        })
    }
    fn bot(&self) -> bool {
        self.kind == "Bot" || self.login.ends_with("[bot]")
    }
}
#[derive(Deserialize)]
struct ReviewRequest {
    #[serde(rename = "requestedReviewer")]
    reviewer: Option<Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphReview {
    id: String,
    body: String,
    author: Actor,
    submitted_at: Option<Time>,
    state: String,
}
#[derive(Deserialize)]
struct GraphCommit {
    commit: CommitDates,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitDates {
    committed_date: Option<Time>,
    authored_date: Option<Time>,
}
struct ReviewHistory {
    threads: Vec<Thread>,
    handled_reviews: BTreeSet<String>,
    description_at: Option<Time>,
    editor: Option<Actor>,
}

type Key = (String, String, u64);

const ACTORS: &str = "fragment ActorFields on Actor { login url __typename ... on User { databaseId } ... on Bot { databaseId } ... on Mannequin { databaseId } ... on Organization { databaseId } }";
const PAGE: &str = "totalCount pageInfo { hasNextPage endCursor }";
const COMMENT_FIELDS: &str = "body createdAt url author { ...ActorFields }";
const REVIEW_COMMENT_FIELDS: &str =
    "body createdAt url author { ...ActorFields } pullRequestReview { id }";
const REVIEW_FIELDS: &str = "id body submittedAt state author { ...ActorFields }";
const COMMIT_FIELDS: &str = "commit { committedDate authoredDate }";
const REQUEST_FIELDS: &str = "requestedReviewer { __typename ... on User { ...ActorFields } ... on Bot { ...ActorFields } ... on Mannequin { ...ActorFields } }";

fn thread_fields() -> String {
    // A smaller nested page avoids multiplying the GraphQL node cost by 100²
    // for every PR. Every remaining comment page is still fetched below.
    format!("id isResolved comments(first:20) {{ nodes {{ {REVIEW_COMMENT_FIELDS} }} {PAGE} }}")
}

fn oversized(errors: &[Value]) -> bool {
    !errors.is_empty()
        && errors.iter().all(|error| {
            matches!(
                error
                    .get("type")
                    .or_else(|| error.pointer("/extensions/code"))
                    .and_then(Value::as_str),
                Some("MAX_NODE_LIMIT_EXCEEDED" | "RESOURCE_LIMITS_EXCEEDED" | "QUERY_TOO_COMPLEX")
            )
        })
}

fn graph_data(response: Graph<Value>) -> Result<Value> {
    if response.errors.is_some_and(|errors| !errors.is_empty()) {
        return Err(Error::upstream());
    }
    response
        .data
        .filter(|data| !data.is_null())
        .ok_or_else(Error::upstream)
}

#[async_trait]
impl Forge for Github {
    fn source_id(&self) -> &str {
        &self.config.id
    }
    async fn identity(&self) -> Result<Identity> {
        let me: User = self.http.get("user", &[]).await?;
        Ok(Identity {
            source_id: self.config.id.clone(),
            kind: self.config.kind,
            account_id: me.id.to_string(),
            display_name: me.person().display_name,
            username: me.login.clone(),
            profile_url: me.person().url,
        })
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        let me: User = self.http.get("user", &[]).await?;
        progress.discovering();
        let mut found = BTreeMap::new();
        for qualifier in ["author", "review-requested", "mentions"] {
            let q = format!("is:pr is:open {qualifier}:{}", me.login);
            let mut received = 0_u64;
            for page in 1..=self.http.max_pages() {
                let response: Search = self
                    .http
                    .get(
                        "search/issues",
                        &[
                            ("q", q.clone()),
                            ("per_page", self.http.page_size().to_string()),
                            ("page", page.to_string()),
                        ],
                    )
                    .await?;
                if response.incomplete_results || response.total_count > 1000 {
                    return Err(Error::new(
                        http::StatusCode::BAD_GATEWAY,
                        "search_limit",
                        "GitHub search was incomplete or exceeded its 1000-result limit; previous data was retained.",
                    ));
                }
                // Search exposes an actual total: a short/clamped page alone
                // cannot prove the inventory is complete.
                received = received.saturating_add(
                    u64::try_from(response.items.len()).map_err(|_| Error::upstream())?,
                );
                let done = received >= response.total_count;
                if !done && response.items.is_empty() {
                    return Err(Error::upstream());
                }
                for item in response.items {
                    let u = url::Url::parse(&item.repository_url).map_err(|_| Error::upstream())?;
                    let parts: Vec<_> = u
                        .path_segments()
                        .ok_or_else(Error::upstream)?
                        .filter(|s| !s.is_empty())
                        .collect();
                    if parts.len() < 2 {
                        return Err(Error::upstream());
                    }
                    let owner = parts[parts.len() - 2];
                    let repo = parts[parts.len() - 1];
                    if self.config.includes(&format!("{owner}/{repo}")) {
                        found.insert((owner.to_owned(), repo.to_owned(), item.number), ());
                    }
                }
                if done {
                    break;
                }
                if page == self.http.max_pages() {
                    return Err(Error::new(
                        http::StatusCode::BAD_GATEWAY,
                        "pagination_limit",
                        "GitHub discovery exceeded the configured page bound; previous data was retained.",
                    ));
                }
            }
        }
        progress.fetching(Some(
            u64::try_from(found.len()).map_err(|_| Error::upstream())?,
        ));
        let mut data=SourceData {coverage_notes:vec!["GitHub discovery uses author, requested-reviewer and mention search, limited by credential permissions, GitHub indexing and the 1000-result search ceiling. Team-only review requests and inaccessible/deleted history are not guaranteed.".into()],identity:Some(Identity{source_id:self.config.id.clone(),kind:Kind::Github,account_id:me.id.to_string(),display_name:me.person().display_name,username:me.login.clone(),profile_url:me.person().url}),..Default::default()};
        // Count active PRs, not just batches: a wide batch must not multiply
        // the configured item-concurrency bound. No detached tasks are spawned.
        let concurrent = self.http.limits().max_concurrent_items;
        let batch_size = self.http.limits().github_batch_size.min(concurrent);
        let keys: Vec<_> = found.into_keys().collect();
        let progress = &progress;
        let login = &me.login;
        let mut batches = stream::iter(keys.chunks(batch_size).map(<[Key]>::to_vec))
            .map(|keys| async move {
                let pulls = self.batch(&keys).await?;
                stream::iter(pulls)
                    .map(|(key, pull)| async move {
                        let mut notes = vec![];
                        let pull = self.hydrate(&key, pull, login, &mut notes).await?;
                        progress.item_completed();
                        Ok::<_, Error>((key, pull, notes))
                    })
                    .buffer_unordered(batch_size)
                    .try_collect::<Vec<_>>()
                    .await
            })
            .buffer_unordered((concurrent / batch_size).max(1));
        let mut evaluated = vec![];
        while let Some(batch) = batches.try_next().await? {
            evaluated.extend(batch);
        }
        evaluated.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, pull, notes) in evaluated {
            data.pull_requests.push(pull);
            data.coverage_notes.extend(notes);
        }
        data.upstream_issues = crate::upstream::discover(
            &self.config,
            &self.http,
            data.identity.as_ref().ok_or_else(Error::upstream)?,
            progress,
        )
        .await?;
        Ok(data)
    }
    async fn issue(
        &self,
        route: &crate::upstream::IssueRoute,
        identity: &Identity,
    ) -> Result<UpstreamIssue> {
        crate::upstream::fetch(&self.config, &self.http, route, identity).await
    }
}
impl Github {
    /// One actual GraphQL document covers multiple PRs, including mixed repos.
    /// Only explicit resource/complexity errors allow splitting; permission,
    /// missing members, rate limits and read failures always fail the source.
    async fn batch(&self, keys: &[Key]) -> Result<Vec<(Key, GraphPull)>> {
        let mut pending = vec![keys];
        let mut out = Vec::with_capacity(keys.len());
        while let Some(keys) = pending.pop() {
            let mut declarations = String::from("$first:Int!");
            let mut selections = String::new();
            let mut variables = json!({"first":self.http.page_size().min(100)});
            for (i, (owner, repo, number)) in keys.iter().enumerate() {
                // All upstream strings are variables, never query text.
                declarations.push_str(&format!(
                    ",$owner{i}:String!,$repo{i}:String!,$number{i}:Int!"
                ));
                selections.push_str(&format!("r{i}:repository(owner:$owner{i},name:$repo{i}) {{ pullRequest(number:$number{i}) {{ ...PullFields }} }}"));
                variables[format!("owner{i}")] = json!(owner);
                variables[format!("repo{i}")] = json!(repo);
                variables[format!("number{i}")] = json!(number);
            }
            let query = format!(
                "query PullBatch({declarations}) {{ {selections} }} \
                 fragment PullFields on PullRequest {{ \
                 id number title body url author {{ ...ActorFields }} isDraft createdAt updatedAt lastEditedAt editor {{ ...ActorFields }} \
                 assignees(first:$first) {{ nodes {{ ...ActorFields }} {PAGE} }} \
                 reviewRequests(first:$first) {{ nodes {{ {REQUEST_FIELDS} }} {PAGE} }} \
                 labels(first:$first) {{ nodes {{ name }} {PAGE} }} \
                 comments(first:$first) {{ nodes {{ {COMMENT_FIELDS} }} {PAGE} }} \
                 reviews(first:$first) {{ nodes {{ {REVIEW_FIELDS} }} {PAGE} }} \
                 commits(first:$first) {{ nodes {{ {COMMIT_FIELDS} }} {PAGE} }} \
                 reviewThreads(first:$first) {{ nodes {{ {} }} {PAGE} }} }} {ACTORS}",
                thread_fields(),
            );
            let response: Graph<Value> = self.http.query(&query, variables).await?;
            if let Some(errors) = &response.errors
                && oversized(errors)
                && keys.len() > 1
            {
                let (left, right) = keys.split_at(keys.len() / 2);
                pending.push(right);
                pending.push(left);
                continue;
            }
            let value = graph_data(response)?;
            for (i, key) in keys.iter().enumerate() {
                let pull: GraphPull = serde_json::from_value(
                    value
                        .get(format!("r{i}"))
                        .and_then(|r| r.get("pullRequest"))
                        .filter(|p| !p.is_null())
                        .ok_or_else(Error::upstream)?
                        .clone(),
                )
                .map_err(|_| Error::upstream())?;
                if pull.number != key.2 || pull.id.is_empty() {
                    return Err(Error::upstream());
                }
                out.push((key.clone(), pull));
            }
        }
        Ok(out)
    }

    /// Drain one connection within its own page/cursor budget. Repeated cursors,
    /// null nodes, failed pages and inconsistent totals cannot silently truncate.
    async fn complete<T: DeserializeOwned>(
        &self,
        id: &str,
        node_type: &str,
        field: &str,
        fields: &str,
        mut connection: Connection<T>,
    ) -> Result<Vec<T>> {
        let mut seen = BTreeSet::new();
        let mut pages = 1;
        while connection.page_info.has_next_page {
            let after = connection
                .page_info
                .end_cursor
                .take()
                .filter(|c| !c.is_empty())
                .ok_or_else(Error::upstream)?;
            if pages >= self.http.max_pages() || !seen.insert(after.clone()) {
                return Err(Error::upstream());
            }
            let actors = if fields.contains("ActorFields") {
                ACTORS
            } else {
                ""
            };
            let query = format!(
                "query PullConnection($id:ID!,$after:String!,$first:Int!) {{ node(id:$id) {{ ... on {node_type} {{ {field}(first:$first,after:$after) {{ nodes {{ {fields} }} {PAGE} }} }} }} }} {actors}"
            );
            let response: Graph<Value> = self
                .http
                .query(
                    &query,
                    json!({"id":id,"after":after,"first":self.http.page_size().min(100)}),
                )
                .await?;
            let value = graph_data(response)?;
            let next: Connection<T> = serde_json::from_value(
                value
                    .get("node")
                    .and_then(|n| n.get(field))
                    .ok_or_else(Error::upstream)?
                    .clone(),
            )
            .map_err(|_| Error::upstream())?;
            if next.total_count != connection.total_count
                || (next.page_info.has_next_page && next.nodes.is_empty())
            {
                return Err(Error::upstream());
            }
            connection.nodes.extend(next.nodes);
            connection.page_info = next.page_info;
            pages += 1;
        }
        if u64::try_from(connection.nodes.len()).map_err(|_| Error::upstream())?
            != connection.total_count
        {
            return Err(Error::upstream());
        }
        Ok(connection.nodes)
    }

    async fn hydrate(
        &self,
        key: &Key,
        graph: GraphPull,
        me: &str,
        notes: &mut Vec<String>,
    ) -> Result<PullRequest> {
        let (owner, repo, number) = key;
        // Keep REST timeline history: it supplies the upstream request actor and
        // request timestamp without conflating review bodies with flat replies.
        let timeline_path = format!(
            "repos/{}/{}/issues/{number}/timeline",
            segment(owner),
            segment(repo)
        );
        let requests_and_timeline = async {
            let requests = self
                .complete(
                    &graph.id,
                    "PullRequest",
                    "reviewRequests",
                    REQUEST_FIELDS,
                    graph.review_requests,
                )
                .await?;
            let mut reviewers = vec![];
            for request in requests {
                if let Some(reviewer) = request.reviewer {
                    match reviewer.get("__typename").and_then(Value::as_str) {
                        Some("Team") => {} // REST requested_reviewers also excludes teams.
                        Some("User" | "Bot" | "Mannequin") => {
                            let actor: Actor =
                                serde_json::from_value(reviewer).map_err(|_| Error::upstream())?;
                            reviewers.push(actor.user()?);
                        }
                        _ => return Err(Error::upstream()),
                    }
                }
            }
            // History is required only for the currently pending self-review
            // request. Other timeline events do not participate in evaluation.
            let timeline = if reviewers.iter().any(|u| u.login.eq_ignore_ascii_case(me)) {
                self.http.pages::<Timeline>(&timeline_path, &[]).await?
            } else {
                vec![]
            };
            Ok::<_, Error>((reviewers, timeline))
        };
        let thread_fields = thread_fields();
        let (
            assignees,
            (requested_reviewers, timeline),
            labels,
            comments,
            reviews,
            commits,
            threads,
        ) = tokio::try_join!(
            self.complete(
                &graph.id,
                "PullRequest",
                "assignees",
                "...ActorFields",
                graph.assignees
            ),
            requests_and_timeline,
            self.complete(&graph.id, "PullRequest", "labels", "name", graph.labels),
            self.complete(
                &graph.id,
                "PullRequest",
                "comments",
                COMMENT_FIELDS,
                graph.comments
            ),
            self.complete(
                &graph.id,
                "PullRequest",
                "reviews",
                REVIEW_FIELDS,
                graph.reviews
            ),
            self.complete(
                &graph.id,
                "PullRequest",
                "commits",
                COMMIT_FIELDS,
                graph.commits
            ),
            self.complete(
                &graph.id,
                "PullRequest",
                "reviewThreads",
                &thread_fields,
                graph.review_threads
            ),
        )?;
        let p = Pull {
            number: graph.number,
            title: graph.title,
            body: Some(graph.body),
            html_url: graph.url,
            user: graph.author.user()?,
            assignees: assignees.iter().map(Actor::user).collect::<Result<_>>()?,
            requested_reviewers,
            labels,
            draft: Some(graph.is_draft),
            created_at: graph.created_at,
            updated_at: graph.updated_at,
        };
        let comments = comments
            .into_iter()
            .map(|c| {
                Ok(Comment {
                    body: Some(c.body),
                    user: c.author.ok_or_else(Error::upstream)?.user()?,
                    created_at: c.created_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let reviews = reviews
            .into_iter()
            .map(|r| {
                Ok(Review {
                    id: r.id,
                    body: Some(r.body),
                    user: r.author.user()?,
                    submitted_at: r.submitted_at,
                    state: r.state,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Commit graph order is not timestamp order (rebases and cherry-picks).
        let last_commit_at = commits
            .iter()
            .filter_map(|c| c.commit.committed_date.or(c.commit.authored_date))
            .max();
        let url = safe_url(&p.html_url).ok_or_else(Error::upstream)?;
        let mine = p.user.login.eq_ignore_ascii_case(me);
        let history = self
            .review_threads(threads, me, mine, &url, graph.last_edited_at, graph.editor)
            .await?;
        self.evaluate(
            owner,
            repo,
            *number,
            me,
            notes,
            p,
            comments,
            reviews,
            timeline,
            last_commit_at,
            history,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        me: &str,
        notes: &mut Vec<String>,
        p: Pull,
        comments: Vec<Comment>,
        reviews: Vec<Review>,
        timeline: Vec<Timeline>,
        last_commit_at: Option<Time>,
        history: ReviewHistory,
    ) -> Result<PullRequest> {
        let url = safe_url(&p.html_url).ok_or_else(Error::upstream)?;
        let mine = p.user.login.eq_ignore_ascii_case(me);
        let mut flat = Thread {
            id: "conversation".into(),
            url: url.clone(),
            resolved: false,
            events: vec![],
        };
        for c in comments {
            let body = c.body.unwrap_or_default();
            flat.events.push(Event {
                at: c.created_at,
                own: c.user.login.eq_ignore_ascii_case(me),
                bot: c.user.bot(),
                opens: reason(&body, mine, me),
                excerpt: body,
            });
        }
        for r in &reviews {
            if let Some(at) = r.submitted_at
                && !r.user.login.eq_ignore_ascii_case(me)
                && !r.body.as_deref().unwrap_or_default().trim().is_empty()
                && !history.handled_reviews.contains(&r.id)
            {
                let body = r.body.clone().unwrap_or_default();
                flat.events.push(Event {
                    at,
                    own: r.user.login.eq_ignore_ascii_case(me),
                    bot: r.user.bot(),
                    opens: reason(&body, mine, me),
                    excerpt: body,
                });
            }
        }
        let mut threads = vec![flat];
        if p.requested_reviewers
            .iter()
            .any(|u| u.login.eq_ignore_ascii_case(me))
        {
            let request = timeline
                .iter()
                .filter(|e| {
                    e.event == "review_requested"
                        && e.requested_reviewer
                            .as_ref()
                            .is_some_and(|u| u.login.eq_ignore_ascii_case(me))
                })
                .filter_map(|e| {
                    e.created_at
                        .map(|at| (at, e.actor.as_ref().is_some_and(User::bot)))
                })
                .max_by_key(|(at, _)| *at);
            if let Some((at, bot)) = request {
                let mut events = vec![Event {
                    at,
                    own: false,
                    bot,
                    opens: Some(Reason::ReviewRequest),
                    excerpt: "Review requested".into(),
                }];
                events.extend(
                    reviews
                        .iter()
                        .filter(|r| r.user.login.eq_ignore_ascii_case(me) && r.state != "PENDING")
                        .filter_map(|r| {
                            r.submitted_at.map(|at| Event {
                                at,
                                own: true,
                                bot: false,
                                opens: None,
                                excerpt: String::new(),
                            })
                        }),
                );
                threads.push(Thread {
                    id: "review-request".into(),
                    url: url.clone(),
                    resolved: false,
                    events,
                });
            } else {
                notes.push("A GitHub PR has a pending review request, but no upstream request timestamp is accessible. Inspect the PR directly; no timer was fabricated.".into());
            }
        }
        let description_own = history
            .editor
            .as_ref()
            .map(|e| e.login.eq_ignore_ascii_case(me))
            .unwrap_or(mine);
        if !description_own && attention::mentions(p.body.as_deref().unwrap_or_default(), me) {
            threads[0].events.push(Event {
                at: history.description_at.unwrap_or(p.created_at),
                own: false,
                bot: history
                    .editor
                    .as_ref()
                    .map(Actor::bot)
                    .unwrap_or_else(|| p.user.bot()),
                opens: Some(Reason::PrMention),
                excerpt: p.body.clone().unwrap_or_default(),
            });
            notes.push("GitHub description mention timers use the current upstream body revision time, not a fabricated discovery time; older body revision history is unavailable.".into());
        }
        threads.extend(history.threads);
        let description = p.body.unwrap_or_default();
        Ok(PullRequest {
            id: format!("{}:{owner}/{repo}:{number}", self.config.id),
            source_id: self.config.id.clone(),
            forge: Kind::Github,
            number: p.number,
            owner: owner.into(),
            repository: repo.into(),
            title: p.title.clone(),
            url,
            description: description.clone(),
            state: if p.draft.unwrap_or(false) {
                "draft"
            } else {
                "ready"
            }
            .into(),
            author: p.user.person(),
            assignees: people(p.assignees.iter().map(User::person).collect(), me),
            reviewers: people(
                p.requested_reviewers
                    .iter()
                    .chain(
                        reviews
                            .iter()
                            .filter(|r| r.state != "PENDING")
                            .map(|r| &r.user),
                    )
                    .map(User::person)
                    .collect(),
                me,
            ),
            labels: p.labels.into_iter().map(|l| l.name).collect(),
            created_at: p.created_at,
            updated_at: p.updated_at,
            last_commit_at,
            linked_issue_keys: references(&format!("{}\n{description}", p.title)),
            is_mine: mine,
            attention: attention::compute(threads, true),
        })
    }
    async fn review_threads(
        &self,
        threads: Vec<ReviewThread>,
        me: &str,
        mine: bool,
        url: &str,
        description_at: Option<Time>,
        editor: Option<Actor>,
    ) -> Result<ReviewHistory> {
        let mut out = vec![];
        let mut handled_reviews = BTreeMap::new();
        for t in threads {
            let comments = self
                .complete(
                    &t.id,
                    "PullRequestReviewThread",
                    "comments",
                    REVIEW_COMMENT_FIELDS,
                    t.comments,
                )
                .await?;
            let thread_url = comments
                .first()
                .and_then(|c| safe_url(&c.url))
                .unwrap_or_else(|| url.to_owned());
            // A review can comment on several threads, including replies on
            // existing threads. Associate only through upstream IDs, never by
            // reviewer or timestamp, and inspect every comment page first.
            let review_ids: BTreeSet<_> = comments
                .iter()
                .filter_map(|c| c.review.as_ref().map(|r| r.id.clone()))
                .collect();
            let events: Vec<_> = comments
                .into_iter()
                .map(|c| Event {
                    at: c.created_at,
                    own: c
                        .author
                        .as_ref()
                        .is_some_and(|a| a.login.eq_ignore_ascii_case(me)),
                    bot: c.author.as_ref().is_some_and(Actor::bot),
                    opens: reason(&c.body, mine, me),
                    excerpt: c.body,
                })
                .collect();
            let replied_at = events
                .iter()
                .filter(|e| e.own && !e.bot)
                .map(|e| e.at)
                .max();
            // Unlike attention eligibility, answering a review requires handling
            // every associated thread, even comments without a self mention on
            // someone else's PR. Equal timestamps are not subsequent replies.
            let handled = t.is_resolved
                || replied_at.is_some_and(|at| events.iter().all(|e| e.own || e.bot || e.at < at));
            for id in review_ids {
                handled_reviews
                    .entry(id)
                    .and_modify(|all_handled| *all_handled &= handled)
                    .or_insert(handled);
            }
            out.push(Thread {
                id: t.id,
                url: thread_url,
                resolved: t.is_resolved,
                events,
            });
        }
        Ok(ReviewHistory {
            threads: out,
            handled_reviews: handled_reviews
                .into_iter()
                .filter_map(|(id, handled)| handled.then_some(id))
                .collect(),
            description_at,
            editor,
        })
    }
}
fn reason(body: &str, mine: bool, me: &str) -> Option<Reason> {
    if attention::mentions(body, me) {
        Some(Reason::PrMention)
    } else if mine {
        Some(Reason::AuthorFeedback)
    } else {
        None
    }
}
pub(crate) fn segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_bot_users_preserve_rest_usernames_without_double_suffixes() {
        for (kind, login, expected) in [
            ("Bot", "app", "app[bot]"),
            ("Bot", "app[bot]", "app[bot]"),
            ("User", "app", "app"),
        ] {
            let actor = Actor {
                login: login.into(),
                kind: kind.into(),
                database_id: Some(9),
                url: Some("https://github.com/apps/app".into()),
            };
            let person = actor.user().unwrap().person();
            assert_eq!(person.id, "9");
            assert_eq!(person.username, expected);
            assert_eq!(person.display_name, expected);
            assert_eq!(person.url.as_deref(), Some("https://github.com/apps/app"));
        }
    }
}
