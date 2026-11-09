//! Typed Jira Cloud reads and explicit, separately gated writes.
use crate::{
    attention::{self, Event, Thread},
    cache::SourceData,
    config::{JiraConfig, Upstream, project_key, safe_url},
    error::{Error, Result},
    model::*,
    request_budget::{Bucket, RequestBudget},
    transport::{bounded_body, http_client},
};
use futures_util::{StreamExt, TryStreamExt, stream};
use http::{HeaderValue, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;
mod discovery;
pub use discovery::Metadata;

pub struct Jira {
    pub config: JiraConfig,
    client: reqwest::Client,
    token: Zeroizing<String>,
    limits: Upstream,
    conditional: Mutex<BTreeMap<String, (HeaderValue, bytes::Bytes)>>,
    budget: Arc<RequestBudget>,
    principal: Mutex<Option<User>>,
    cache: std::sync::OnceLock<Arc<crate::cache::Cache>>,
    metadata_error: Mutex<Option<Error>>,
}
#[derive(Clone, Deserialize)]
struct User {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "emailAddress")]
    email: Option<String>,
    #[serde(rename = "accountType", default)]
    account_type: String,
    #[serde(default)]
    active: bool,
}
impl User {
    fn person(&self) -> Person {
        Person {
            id: self.account_id.clone(),
            username: self
                .email
                .clone()
                .unwrap_or_else(|| self.display_name.clone()),
            display_name: self.display_name.clone(),
            url: None,
        }
    }
    fn bot(&self) -> bool {
        self.account_type == "app"
    }
    fn eligible(&self) -> bool {
        self.active && self.account_type == "atlassian" && !self.account_id.is_empty()
    }
}
#[derive(Deserialize)]
struct Search {
    issues: Vec<UpstreamIssue>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "isLast")]
    is_last: bool,
}
#[derive(Deserialize)]
struct UpstreamIssue {
    id: String,
    key: String,
    fields: Fields,
}
#[derive(Deserialize)]
struct Fields {
    summary: String,
    description: Option<Value>,
    project: NamedProject,
    issuetype: Named,
    status: Status,
    reporter: Option<User>,
    assignee: Option<User>,
    created: Time,
    updated: Time,
    #[serde(flatten)]
    custom: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
struct Named {
    name: String,
}
#[derive(Deserialize)]
struct NamedProject {
    key: String,
}
#[derive(Deserialize)]
struct Status {
    name: String,
}
#[derive(Deserialize)]
struct CommentPage {
    comments: Vec<Comment>,
    total: u64,
    #[serde(rename = "startAt")]
    start_at: u64,
    #[serde(rename = "maxResults")]
    max_results: u64,
}
#[derive(Deserialize)]
struct Comment {
    id: String,
    body: Value,
    author: User,
    created: Time,
    updated: Option<Time>,
}
#[derive(Deserialize)]
struct SprintPage {
    values: Vec<UpstreamSprint>,
    #[serde(rename = "isLast")]
    is_last: bool,
    #[serde(rename = "startAt")]
    start_at: u64,
    #[serde(rename = "maxResults")]
    max_results: u64,
}
#[derive(Deserialize)]
struct UpstreamSprint {
    id: u64,
    name: String,
    state: String,
    #[serde(rename = "startDate")]
    start_date: Option<Time>,
    #[serde(rename = "endDate")]
    end_date: Option<Time>,
}
#[derive(Deserialize)]
struct Weblink {
    object: LinkObject,
}
#[derive(Deserialize)]
struct LinkObject {
    url: String,
}
#[derive(Deserialize)]
struct ChangelogPage {
    values: Vec<History>,
    #[serde(rename = "isLast")]
    is_last: bool,
    #[serde(rename = "startAt")]
    start_at: u64,
    #[serde(rename = "maxResults")]
    max_results: u64,
}
#[derive(Deserialize)]
struct History {
    created: Time,
    items: Vec<Change>,
    author: Option<User>,
}
#[derive(Deserialize)]
struct Change {
    field: String,
}
#[derive(Deserialize)]
struct Transitions {
    transitions: Vec<Transition>,
}
#[derive(Deserialize)]
struct Transition {
    id: String,
    to: StatusChoice,
}
#[derive(Serialize)]
struct CreateBody {
    fields: BTreeMap<String, Value>,
}

impl Jira {
    pub async fn new(config: &JiraConfig, limits: &Upstream) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            config: config.clone(),
            client: http_client(limits)?,
            token: config.token.resolve().await?,
            limits: limits.clone(),
            conditional: Mutex::new(BTreeMap::new()),
            budget: RequestBudget::shared(&config.base_url, limits)?,
            principal: Mutex::new(None),
            cache: std::sync::OnceLock::new(),
            metadata_error: Mutex::new(None),
        }))
    }
    pub fn project(&self, key: &str) -> Result<()> {
        if self.config.projects.iter().any(|p| p == key) {
            Ok(())
        } else {
            Err(discovery::invalid_choice())
        }
    }
    async fn request<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        params: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<T> {
        let mut url = url::Url::parse(&format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
        .map_err(|_| Error::upstream())?;
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        let cached = if method == Method::GET {
            self.conditional.lock().await.get(url.as_str()).cloned()
        } else {
            None
        };
        let mut request = self
            .client
            .request(method.clone(), url.clone())
            .basic_auth(&self.config.email, Some(&*self.token))
            .header("Accept", "application/json");
        if let Some((etag, _)) = &cached {
            request = request.header(http::header::IF_NONE_MATCH, etag);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let _permit = self.budget.acquire(Bucket::General).await?;
        let response = request.send().await.map_err(|_| Error::upstream())?;
        if let Some(error) =
            self.budget
                .observe_headers(Bucket::General, response.status(), response.headers())
        {
            return Err(error);
        }
        if response.status() == StatusCode::NOT_MODIFIED {
            return serde_json::from_slice(&cached.ok_or_else(Error::upstream)?.1)
                .map_err(|_| Error::upstream());
        }
        if !response.status().is_success() {
            return Err(Error::upstream());
        }
        let etag = response.headers().get(http::header::ETAG).cloned();
        let bytes = bounded_body(response, self.limits.max_response_bytes).await?;
        // Jira transitions and link writes commonly have an empty 204 response.
        let value = if bytes.is_empty() {
            serde_json::from_value(Value::Null)
        } else {
            serde_json::from_slice(&bytes)
        }
        .map_err(|_| Error::upstream())?;
        if method == Method::GET
            && bytes.len() <= 16 * 1024 * 1024
            && let Some(etag) = etag
        {
            let mut cache = self.conditional.lock().await;
            let used: usize = cache.values().map(|(_, body)| body.len()).sum();
            if cache.len() >= 2048 || used.saturating_add(bytes.len()) > 16 * 1024 * 1024 {
                cache.clear();
            }
            cache.insert(url.to_string(), (etag, bytes));
        }
        Ok(value)
    }
    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        self.request(Method::GET, path, params, None::<&()>).await
    }
    async fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.request(Method::POST, path, &[], Some(body)).await
    }
    pub async fn discover(&self) -> Result<SourceData> {
        self.discover_with_progress(crate::sync::Progress::default())
            .await
    }
    pub async fn discover_with_progress(
        &self,
        progress: crate::sync::Progress,
    ) -> Result<SourceData> {
        let result = self.read_source(progress).await;
        if let Err(error) = &result {
            self.choice_failure(error.clone()).await;
        }
        result
    }
    pub(crate) async fn choice_failure(&self, error: Error) {
        *self.metadata_error.lock().await = Some(error);
    }
    async fn read_source(&self, progress: crate::sync::Progress) -> Result<SourceData> {
        let identity = self.identity().await?;
        let mut metadata = self.read_metadata(&identity.account_id).await?;
        let candidates = match self.group_members().await? {
            Some(members) => members,
            None => self.all_human_accounts().await?,
        };
        for key in &self.config.projects {
            metadata
                .assignees
                .insert(key.clone(), self.read_assignees(key, &candidates).await?);
        }
        let mut data=SourceData{identity:Some(identity),coverage_notes:vec!["Jira discovery uses separate fully paginated enhanced JQL searches for assigned issues in configured projects and description/comment mention candidates across all accessible projects, regardless of assignee. Only status Closed is excluded, not resolution or Done category; there is no date cutoff or unfiltered fallback. Search indexing, permissions and deleted history may omit mentions. Explicit ADF mentions and upstream replies determine attention. Mention timers use current description/comment revision timestamps when edit history is unavailable; own replies clear at comment creation timestamps.".into()],..Default::default()};
        data.sprints = self.sprints().await?;
        metadata.sprints = data.sprints.clone();
        data.jira_choices = Some(metadata);
        if self.config.projects.is_empty() {
            return Ok(data);
        }
        let keys = self
            .config
            .projects
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(",");
        let assigned_jql = format!(
            "project in ({keys}) AND assignee = currentUser() AND status != \"Closed\" ORDER BY key ASC"
        );
        let mention_jql = "status != \"Closed\" AND (description ~ currentUser() OR comment ~ currentUser()) ORDER BY key ASC";
        let custom = data
            .jira_choices
            .as_ref()
            .ok_or_else(Error::internal)?
            .sprint_field
            .clone()
            .unwrap_or_default();
        let fields = format!(
            "summary,description,project,issuetype,status,reporter,assignee,created,updated,{custom}"
        );
        // Complete both searches and deduplicate stable upstream IDs before any
        // per-issue history reads. A failure never yields a partial source snapshot.
        progress.discovering();
        let mut candidates = BTreeMap::new();
        for jql in [assigned_jql.as_str(), mention_jql] {
            self.search(jql, &fields, &mut candidates).await?;
        }
        let candidates: Vec<_> = candidates
            .into_values()
            .filter(|up| !up.fields.status.name.eq_ignore_ascii_case("Closed"))
            .collect();
        progress.fetching(Some(
            u64::try_from(candidates.len()).map_err(|_| Error::upstream())?,
        ));
        // Unordered polling observes any required failure immediately, even when an
        // earlier candidate is slow. Dropping the stream cancels all borrowed reads.
        let mut evaluated: Vec<_> = stream::iter(candidates.into_iter().enumerate())
            .map(|(index, up)| {
                let progress = &progress;
                let custom = &custom;
                async move {
                    let (issue, links) = self.issue(up, custom).await?;
                    progress.item_completed();
                    Ok::<_, Error>((index, issue, links))
                }
            })
            .buffer_unordered(self.limits.max_concurrent_items)
            .try_collect()
            .await?;
        evaluated.sort_by_key(|(index, _, _)| *index);
        for (_, issue, links) in evaluated {
            if (issue.is_assigned_to_me && self.project(&issue.project_key).is_ok())
                || issue.attention.is_some()
            {
                data.issue_weblinks
                    .extend(links.into_iter().map(|url| (issue.key.clone(), url)));
                data.issues.push(issue);
            }
        }
        Ok(data)
    }
    async fn search(
        &self,
        jql: &str,
        fields: &str,
        candidates: &mut BTreeMap<String, UpstreamIssue>,
    ) -> Result<()> {
        let mut cursor: Option<String> = None;
        let mut seen_cursors = BTreeSet::new();
        for page in 0..self.limits.max_pages {
            let mut params = vec![
                ("jql", jql.to_owned()),
                ("fields", fields.to_owned()),
                ("maxResults", self.limits.page_size.to_string()),
            ];
            if let Some(c) = &cursor {
                params.push(("nextPageToken", c.clone()));
            }
            let response: Search = self.get("rest/api/3/search/jql", &params).await?;
            for issue in response.issues {
                if issue.id.is_empty() {
                    return Err(Error::upstream());
                }
                candidates.entry(issue.id.clone()).or_insert(issue);
            }
            if response.is_last {
                return Ok(());
            }
            let next = response
                .next_page_token
                .filter(|token| !token.is_empty())
                .ok_or_else(Error::upstream)?;
            if !seen_cursors.insert(next.clone()) || page + 1 == self.limits.max_pages {
                return Err(Error::new(
                    StatusCode::BAD_GATEWAY,
                    "pagination_limit",
                    "Jira cursor search exceeded the configured bound; previous data was retained.",
                ));
            }
            cursor = Some(next);
        }
        Err(Error::upstream())
    }
    async fn issue(
        &self,
        up: UpstreamIssue,
        sprint_field: &str,
    ) -> Result<(JiraIssue, Vec<String>)> {
        let me = self.account_id().await?;
        if !valid_issue_key(&up.key)
            || !up
                .key
                .rsplit_once('-')
                .is_some_and(|(project, _)| project == up.fields.project.key)
        {
            return Err(Error::upstream());
        }
        let root = format!("rest/api/3/issue/{}", up.key);
        let url = format!(
            "{}/browse/{}",
            self.config.base_url.trim_end_matches('/'),
            up.key
        );
        let description = up
            .fields
            .description
            .as_ref()
            .map(adf_text)
            .unwrap_or_default();
        let revision = async {
            if up
                .fields
                .description
                .as_ref()
                .is_some_and(|d| adf_mentions(d, &me))
            {
                self.description_revision(&root, up.fields.created, up.fields.reporter.clone())
                    .await
                    .map(Some)
            } else {
                Ok(None)
            }
        };
        let links_path = format!("{root}/remotelink");
        let (revision, comments, weblinks): (_, _, Vec<Weblink>) = tokio::try_join!(
            revision,
            self.comment_events(&root, &me),
            self.get(&links_path, &[]),
        )?;
        let mut events = vec![];
        if let Some((at, author)) = revision {
            // Editing one's own description is not a subsequent Jira comment and
            // therefore must not clear mentions received in other comments.
            if !author.as_ref().is_some_and(|u| u.account_id == me) {
                events.push(Event {
                    at,
                    own: false,
                    bot: author.as_ref().is_some_and(User::bot),
                    opens: Some(Reason::JiraMention),
                    excerpt: description.clone(),
                });
            }
        }
        events.extend(comments);
        let sprint_ids = up
            .fields
            .custom
            .get(sprint_field)
            .map(sprint_ids)
            .unwrap_or_default();
        let is_assigned_to_me = up
            .fields
            .assignee
            .as_ref()
            .is_some_and(|u| u.account_id == me);
        Ok((
            JiraIssue {
                id: up.id,
                source_id: "jira".into(),
                key: up.key.clone(),
                url: url.clone(),
                title: up.fields.summary,
                description,
                project_key: up.fields.project.key,
                issue_type: up.fields.issuetype.name,
                status: up.fields.status.name,
                reporter: up.fields.reporter.map(|u| u.person()),
                assignees: up.fields.assignee.into_iter().map(|u| u.person()).collect(),
                is_assigned_to_me,
                created_at: up.fields.created,
                updated_at: up.fields.updated,
                sprint_ids,
                linked_pr_ids: vec![],
                attention: attention::compute(
                    vec![Thread {
                        id: format!("jira:{}", up.key),
                        url,
                        resolved: false,
                        events,
                    }],
                    true,
                ),
            },
            weblinks
                .into_iter()
                .filter_map(|l| safe_url(&l.object.url))
                .collect(),
        ))
    }
    async fn comment_events(&self, root: &str, me: &str) -> Result<Vec<Event>> {
        let mut events = vec![];
        let mut start = 0;
        for page in 0..self.limits.max_pages {
            let response: CommentPage = self
                .get(
                    &format!("{root}/comment"),
                    &[
                        ("startAt", start.to_string()),
                        ("maxResults", self.limits.page_size.to_string()),
                    ],
                )
                .await?;
            let len = u64::try_from(response.comments.len()).map_err(|_| Error::upstream())?;
            for c in response.comments {
                let _ = c.id;
                let mentioned = adf_mentions(&c.body, me);
                let own = c.author.account_id == me;
                events.push(Event {
                    at: if mentioned && !own {
                        c.updated.unwrap_or(c.created)
                    } else {
                        c.created
                    },
                    own,
                    bot: c.author.bot(),
                    opens: mentioned.then_some(Reason::JiraMention),
                    excerpt: adf_text(&c.body),
                });
            }
            if response.start_at.saturating_add(len) >= response.total {
                return Ok(events);
            }
            if len == 0 || response.max_results == 0 || page + 1 == self.limits.max_pages {
                return Err(Error::upstream());
            }
            start = response.start_at.saturating_add(len);
        }
        Err(Error::upstream())
    }
    async fn description_revision(
        &self,
        root: &str,
        created: Time,
        reporter: Option<User>,
    ) -> Result<(Time, Option<User>)> {
        let mut start = 0;
        let mut at = created;
        let mut author = reporter;
        for page in 0..self.limits.max_pages {
            let response: ChangelogPage = self
                .get(
                    &format!("{root}/changelog"),
                    &[
                        ("startAt", start.to_string()),
                        ("maxResults", self.limits.page_size.to_string()),
                    ],
                )
                .await?;
            let count = u64::try_from(response.values.len()).map_err(|_| Error::upstream())?;
            for h in response.values {
                if h.created >= at && h.items.iter().any(|i| i.field == "description") {
                    at = h.created;
                    author = h.author;
                }
            }
            if response.is_last {
                return Ok((at, author));
            }
            if count == 0 || response.max_results == 0 || page + 1 == self.limits.max_pages {
                return Err(Error::upstream());
            }
            start = response.start_at.saturating_add(count);
        }
        Err(Error::upstream())
    }
    pub async fn sprints(&self) -> Result<Vec<Sprint>> {
        let mut out = vec![];
        let pattern = self
            .config
            .sprint_pattern()
            .map_err(|_| Error::internal())?;
        let board = self.config.board_id;
        let mut start = 0;
        for page in 0..self.limits.max_pages {
            let response: SprintPage = self
                .get(
                    &format!("rest/agile/1.0/board/{board}/sprint"),
                    &[
                        ("state", "active,future".into()),
                        ("startAt", start.to_string()),
                        ("maxResults", self.limits.page_size.to_string()),
                    ],
                )
                .await?;
            let count = u64::try_from(response.values.len()).map_err(|_| Error::upstream())?;
            if response.start_at != start {
                return Err(discovery::failed());
            }
            for s in response.values {
                if matches!(s.state.as_str(), "active" | "future") {
                    out.push(Sprint {
                        id: s.id.to_string(),
                        board_id: board,
                        team: None,
                        default_eligible: s.state == "active" && pattern.is_match(&s.name),
                        name: s.name,
                        state: s.state,
                        board_url: format!(
                            "{}/secure/RapidBoard.jspa?rapidView={board}",
                            self.config.base_url.trim_end_matches('/')
                        ),
                        start_at: s.start_date,
                        end_at: s.end_date,
                    });
                }
            }
            if response.is_last {
                break;
            }
            if count == 0 || response.max_results == 0 || page + 1 == self.limits.max_pages {
                return Err(Error::upstream());
            }
            start = response.start_at.saturating_add(count);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.board_id.cmp(&b.board_id)));
        out.dedup_by(|a, b| a.id == b.id && a.board_id == b.board_id);
        Ok(out)
    }
    /// Explicit refresh-time discovery; API consumers use the committed choices.
    pub async fn discover_assignees(
        &self,
        key: &str,
        purpose: Purpose,
        query: &str,
    ) -> Result<Vec<JiraAssignee>> {
        self.project(key)?;
        let me = self.account_id().await?;
        if query.chars().count() > 200 {
            return Err(Error::invalid());
        }
        let groups = self.group_members().await?;
        let candidates = match groups {
            Some(members) => members,
            None => self.all_human_accounts().await?,
        };
        let mut out = self.read_assignees(key, &candidates).await?;
        let query = query.to_lowercase();
        out.retain(|p| {
            (purpose != Purpose::Review || p.id != me)
                && (p.display_name.to_lowercase().contains(&query)
                    || p.username.to_lowercase().contains(&query)
                    || p.email
                        .as_ref()
                        .is_some_and(|e| e.to_lowercase().contains(&query)))
        });
        Ok(out)
    }
    async fn read_assignees(
        &self,
        key: &str,
        candidates: &BTreeSet<String>,
    ) -> Result<Vec<JiraAssignee>> {
        let bound = u64::from(self.limits.max_pages) * u64::from(self.limits.page_size);
        if u64::try_from(candidates.len()).map_err(|_| discovery::failed())? > bound {
            return Err(discovery::failed());
        }
        let mut out: Vec<JiraAssignee> =
            stream::iter(candidates.iter().cloned().collect::<Vec<_>>())
                .map(|account| async move {
                    // The range-based form only scans the first 1000 users and filters
                    // after scanning. Only this documented accountId lookup can prove
                    // whether an individual candidate is assignable in a large tenant.
                    let users: Vec<User> = self
                        .get(
                            "rest/api/3/user/assignable/search",
                            &[("project", key.into()), ("accountId", account.clone())],
                        )
                        .await
                        .map_err(discovery::discovery_error)?;
                    if users.len() > 1 || users.iter().any(|u| u.account_id != account) {
                        return Err(discovery::failed());
                    }
                    Ok::<_, Error>(users.into_iter().next().filter(User::eligible).map(|user| {
                        JiraAssignee {
                            person: user.person(),
                            email: user.email,
                        }
                    }))
                })
                .buffer_unordered(self.limits.max_concurrent_items)
                .try_collect::<Vec<_>>()
                .await?
                .into_iter()
                .flatten()
                .collect();
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.username.cmp(&b.username))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }
    pub fn validate_create(&self, r: &CreateIssueRequest, sprints: &[Sprint]) -> Result<()> {
        self.project(&r.project_key)?;
        if r.pull_request_id.is_empty()
            || r.issue_type.trim().is_empty()
            || r.title.trim().is_empty()
            || r.title.chars().count() > 255
            || r.description.chars().count() > 100000
            || r.story_points.is_some_and(|n| !n.is_finite() || n < 0.0)
        {
            return Err(Error::invalid());
        }
        if let Some(id) = &r.sprint_id
            && (id.parse::<u64>().is_err()
                || !sprints
                    .iter()
                    .any(|s| &s.id == id && matches!(s.state.as_str(), "active" | "future")))
        {
            return Err(discovery::invalid_choice());
        }
        if r.status
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.chars().count() > 255)
        {
            return Err(discovery::invalid_choice());
        }
        if r.assignee_id
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 200)
        {
            return Err(discovery::invalid_choice());
        }
        Ok(())
    }
    pub async fn create(
        &self,
        r: &CreateIssueRequest,
        pr: &PullRequest,
        _sprints: &[Sprint],
        read_only: bool,
    ) -> Result<CreateIssueResult> {
        self.create_linked(r, &pr.id, &pr.url, &pr.title, "pull request", read_only)
            .await
    }
    pub async fn create_upstream_issue(
        &self,
        r: &CreateUpstreamIssueRequest,
        issue: &crate::model::UpstreamIssue,
        read_only: bool,
    ) -> Result<CreateIssueResult> {
        let fields = CreateIssueRequest {
            pull_request_id: r.upstream_issue_id.clone(),
            expected_mode_revision: r.expected_mode_revision.clone(),
            purpose: Purpose::Implementation,
            project_key: r.project_key.clone(),
            issue_type: r.issue_type.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            story_points: r.story_points,
            assignee_id: r.assignee_id.clone(),
            sprint_id: r.sprint_id.clone(),
            team: r.team.clone(),
            status: r.status.clone(),
        };
        self.create_linked(
            &fields,
            &issue.id,
            &issue.url,
            &issue.title,
            "upstream issue",
            read_only,
        )
        .await
    }
    async fn create_linked(
        &self,
        r: &CreateIssueRequest,
        selection_id: &str,
        url: &str,
        title: &str,
        kind: &str,
        read_only: bool,
    ) -> Result<CreateIssueResult> {
        let metadata = self.choices().await?;
        let principal = metadata.account_id.clone();
        let options = self
            .resolve_options(
                &metadata,
                &r.project_key,
                r.purpose,
                Some(&r.issue_type),
                "",
            )
            .await?;
        self.validate_create(r, &options.sprints)?;
        let selected = metadata
            .types
            .get(&r.project_key)
            .and_then(|types| types.get(&r.issue_type))
            .ok_or_else(discovery::invalid_choice)?;
        self.validate_choices(r, &options)?;
        if !r.description.is_empty() && !selected.permits("description") {
            return Err(discovery::invalid_choice());
        }
        // This gate precedes every possible upstream write, including follow-up writes.
        if r.pull_request_id != selection_id || safe_url(url).is_none() {
            return Err(Error::invalid());
        }
        if read_only {
            return Ok(CreateIssueResult {
                outcome: "simulated".into(),
                issue: None,
                message: "Ticket creation simulated (read-only mode).".into(),
                warnings: vec![],
            });
        }
        let mut fields = BTreeMap::from([
            ("project".into(), json!({"key":r.project_key})),
            ("issuetype".into(), json!({"id":selected.id})),
            ("summary".into(), json!(r.title)),
            (
                "assignee".into(),
                r.assignee_id
                    .as_ref()
                    .map(|id| json!({"accountId":id}))
                    .unwrap_or(Value::Null),
            ),
        ]);
        if selected.permits("description") {
            fields.insert("description".into(), encode_adf(&r.description));
        }
        if let (Some(field), Some(points)) = (&selected.points_field, r.story_points) {
            fields.insert(field.clone(), json!(points));
        }
        if let (Some(field), Some(id)) = (&selected.sprint_field, &r.sprint_id) {
            fields.insert(
                field.clone(),
                json!(id.parse::<u64>().map_err(|_| Error::invalid())?),
            );
        }
        if let (Some(field), Some(choice)) = (&selected.team_field, &r.team) {
            fields.insert(
                field.clone(),
                selected
                    .team_values
                    .get(choice)
                    .ok_or_else(discovery::invalid_choice)?
                    .clone(),
            );
        }
        if self.identity().await?.account_id != principal {
            return Err(Error::unavailable());
        }
        let created = self.create_remote(&CreateBody { fields }).await?;
        // Once a real issue was created, never return an error that invites duplicate creation.
        let mut warnings = vec![];
        let Some(key) = created.filter(|k| valid_issue_key(k)) else {
            return Ok(CreateIssueResult{outcome:"created".into(),issue:None,message:"Jira accepted ticket creation but returned no usable issue key. Inspect Jira before retrying.".into(),warnings:vec!["The issue could not be safely linked or transitioned; do not resubmit creation.".into()]});
        };
        let issue = IssueReference {
            key: key.clone(),
            url: format!(
                "{}/browse/{key}",
                self.config.base_url.trim_end_matches('/')
            ),
            title: r.title.clone(),
        };
        // Jira Software Cloud's documented Move issues to sprint endpoint accepts
        // issue keys and works without Sprint being on the create screen.
        // https://developer.atlassian.com/cloud/jira/software/rest/api-group-sprint/#api-rest-agile-1-0-sprint-sprintid-issue-post
        if options.capabilities.sprint.mode == "follow_up"
            && let Some(sprint) = &r.sprint_id
            && self
                .post::<Value, _>(
                    &format!("rest/agile/1.0/sprint/{sprint}/issue"),
                    &json!({"issues":[key]}),
                )
                .await
                .is_err()
        {
            warnings.push("Ticket created, but assigning the explicitly selected sprint failed. Inspect Jira and retry assignment, not ticket creation.".into());
        }
        if self
            .write_link_reference(&key, selection_id, url, title)
            .await
            .is_err()
        {
            warnings.push(format!("Ticket created, but linking the {kind} failed. Inspect Jira and retry linking, not ticket creation."));
        }
        if let Some(status) = &r.status
            && let Some(target) = options.statuses.iter().find(|s| &s.name == status)
            && self.transition(&key, &target.id).await.is_err()
        {
            warnings.push("Ticket created, but the explicitly requested status transition failed. Inspect Jira before retrying the transition.".into());
        }
        Ok(CreateIssueResult {
            outcome: "created".into(),
            issue: Some(issue),
            message: "Ticket created.".into(),
            warnings,
        })
    }
    async fn transition(&self, key: &str, status: &str) -> Result<()> {
        let current: Value = self
            .get(
                &format!("rest/api/3/issue/{key}"),
                &[("fields", "status".into())],
            )
            .await?;
        let current_id = current
            .pointer("/fields/status/id")
            .and_then(Value::as_str)
            .ok_or_else(Error::upstream)?;
        if current_id == status {
            return Ok(());
        }
        let options: Transitions = self
            .get(&format!("rest/api/3/issue/{key}/transitions"), &[])
            .await?;
        let mut transitions = options
            .transitions
            .into_iter()
            .filter(|t| t.to.id == status);
        let transition = transitions.next().ok_or_else(Error::invalid)?;
        if transitions.next().is_some() {
            return Err(Error::invalid());
        }
        let _: Value = self
            .post(
                &format!("rest/api/3/issue/{key}/transitions"),
                &json!({"transition":{"id":transition.id}}),
            )
            .await?;
        Ok(())
    }
    async fn write_link(&self, key: &str, pr: &PullRequest) -> Result<()> {
        self.write_link_reference(key, &pr.id, &pr.url, &pr.title)
            .await
    }
    async fn write_link_reference(
        &self,
        key: &str,
        id: &str,
        url: &str,
        title: &str,
    ) -> Result<()> {
        self.verify_identity().await?;
        let _:Value=self.post(&format!("rest/api/3/issue/{key}/remotelink"),&json!({"globalId":format!("pullboard:{id}"),"object":{"url":url,"title":title},"relationship":"relates to"})).await?;
        Ok(())
    }
    async fn verify_identity(&self) -> Result<()> {
        self.identity().await?;
        Ok(())
    }
    async fn create_remote(&self, body: &CreateBody) -> Result<Option<String>> {
        let _permit = self.budget.acquire(Bucket::General).await?;
        let response=self.client.post(format!("{}/rest/api/3/issue",self.config.base_url.trim_end_matches('/'))).basic_auth(&self.config.email,Some(&*self.token)).json(body).send().await.map_err(|_|Error::new(StatusCode::BAD_GATEWAY,"write_outcome_unknown","Jira creation outcome is unknown. Inspect Jira before retrying to avoid duplicate tickets."))?;
        if let Some(error) =
            self.budget
                .observe_headers(Bucket::General, response.status(), response.headers())
        {
            return Err(error);
        }
        if !response.status().is_success() {
            if response.status().is_server_error()
                || response.status() == StatusCode::REQUEST_TIMEOUT
            {
                return Err(Error::new(
                    StatusCode::BAD_GATEWAY,
                    "write_outcome_unknown",
                    "Jira creation outcome is unknown. Inspect Jira before retrying to avoid duplicate tickets.",
                ));
            }
            return Err(Error::upstream());
        }
        // After an accepted creation even a truncated/malformed body is a successful
        // write with warnings, not an error inviting a duplicate creation.
        let Ok(bytes) = bounded_body(response, self.limits.max_response_bytes).await else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(None);
        };
        Ok(value.get("key").and_then(Value::as_str).map(str::to_owned))
    }
    pub async fn link(
        &self,
        key: &str,
        pr: &PullRequest,
        already: bool,
        read_only: bool,
    ) -> Result<LinkResult> {
        if !valid_issue_key(key) || safe_url(&pr.url).is_none() {
            return Err(Error::invalid());
        }
        let (outcome, message) = if read_only {
            ("simulated", "Linking simulated (read-only mode). ")
        } else if already {
            ("already_linked", "The pull request is already linked.")
        } else {
            self.write_link(key, pr).await?;
            ("linked", "Pull request linked.")
        };
        Ok(LinkResult {
            outcome: outcome.into(),
            issue_key: key.into(),
            pull_request_id: pr.id.clone(),
            message: message.into(),
        })
    }
}
pub fn valid_issue_key(key: &str) -> bool {
    key.rsplit_once('-').is_some_and(|(p, n)| {
        project_key(p)
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && n.parse::<u64>().is_ok_and(|n| n > 0)
    })
}
fn sprint_ids(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|s| {
            s.iter()
                .filter_map(|s| s.get("id").and_then(Value::as_u64).map(|id| id.to_string()))
                .collect()
        })
        .unwrap_or_default()
}
pub fn adf_mentions(v: &Value, id: &str) -> bool {
    (v.get("type").and_then(Value::as_str) == Some("mention")
        && v.get("attrs")
            .and_then(|a| a.get("id"))
            .and_then(Value::as_str)
            == Some(id))
        || v.get("content")
            .and_then(Value::as_array)
            .is_some_and(|nodes| nodes.iter().any(|n| adf_mentions(n, id)))
}
pub fn adf_text(v: &Value) -> String {
    fn visit(v: &Value, out: &mut String) {
        match v.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = v.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            Some("mention") => {
                if let Some(t) = v
                    .get("attrs")
                    .and_then(|a| a.get("text"))
                    .and_then(Value::as_str)
                {
                    out.push_str(t);
                }
            }
            Some("hardBreak") => out.push('\n'),
            _ => {}
        }
        if let Some(nodes) = v.get("content").and_then(Value::as_array) {
            for n in nodes {
                visit(n, out);
            }
        }
        if matches!(
            v.get("type").and_then(Value::as_str),
            Some("paragraph" | "heading" | "listItem" | "codeBlock")
        ) {
            out.push('\n');
        }
    }
    if let Some(s) = v.as_str() {
        return s.to_owned();
    }
    let mut out = String::new();
    visit(v, &mut out);
    out.trim_end().to_owned()
}
pub fn encode_adf(text: &str) -> Value {
    json!({"version":1,"type":"doc","content":text.split('\n').map(|line|if line.is_empty(){json!({"type":"paragraph","content":[]})}else{json!({"type":"paragraph","content":[{"type":"text","text":line}]})}).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adf_mentions_are_explicit_and_text_roundtrips() {
        let d = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"mention","attrs":{"id":"me","text":"@Me"}}]}]});
        assert!(adf_mentions(&d, "me"));
        assert!(!adf_mentions(&encode_adf("@me"), "me"));
        assert_eq!(adf_text(&encode_adf("a\n\nb")), "a\n\nb");
    }
    #[test]
    fn issue_key_validation() {
        assert!(valid_issue_key("DEV-42"));
        assert!(!valid_issue_key("DEV-../../token"));
        assert!(!valid_issue_key("DEV-0"));
    }
}
