//! Same-origin local API. Extraction and routing errors share the wire error envelope.
use crate::{
    ai::Ai,
    cache::Cache,
    config::Config,
    error::{Error, Result},
    forge,
    jira::Jira,
    model::*,
    runtime_mode::Mode,
    sync::Syncer,
    upstream,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, de::DeserializeOwned};
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

pub const BODY_LIMIT: usize = 1024 * 1024;
pub struct App {
    pub config: Config,
    pub cache: Arc<Cache>,
    pub jira: Option<Arc<Jira>>,
    pub ai: Ai,
    pub sync: Arc<Syncer>,
    pub mode: Mode,
    forges: Vec<Arc<dyn forge::Forge>>,
}
impl App {
    pub async fn build(config: Config) -> anyhow::Result<Arc<Self>> {
        config.validate()?;
        let cache = Arc::new(Cache::open(&config.server.database, &config).await?);
        let jira = match &config.jira {
            Some(c) => Some(Jira::new(c, &config.upstream).await?),
            None => None,
        };
        let mut forges = vec![];
        for f in &config.forges {
            forges.push(forge::build(f, &config.upstream).await?);
        }
        let ai = Ai::new(&config.ai, &config.upstream).await?;
        let sync = Syncer::new(
            cache.clone(),
            forges.clone(),
            jira.clone(),
            config.refresh_interval_seconds,
        );
        let mode = Mode::new(config.read_only_test_mode)?;
        Ok(Arc::new(Self {
            config,
            cache,
            jira,
            ai,
            sync,
            mode,
            forges,
        }))
    }
}
pub fn router(app: Arc<App>) -> Router {
    let static_dir = app.config.server.static_directory.clone();
    Router::new()
        .route("/api/health", get(health))
        .route("/api/context", get(context))
        .route(
            "/api/runtime-mode",
            get(runtime_mode).post(change_runtime_mode),
        )
        .route("/api/board", get(board))
        .route("/api/sync", get(sync_status).post(refresh))
        .route("/api/jira/options", get(options))
        .route("/api/ticket-drafts", post(draft))
        .route("/api/jira/issues", post(create))
        .route("/api/jira/issues/{issue_key}/links", post(link))
        .route("/api/upstream-issues/resolve", post(resolve_upstream_issue))
        .route(
            "/api/upstream-issues/ticket-drafts",
            post(draft_upstream_issue),
        )
        .route(
            "/api/upstream-issues/jira-issues",
            post(create_upstream_issue),
        )
        .route("/api", get(not_found).post(not_found))
        .route("/api/", get(not_found).post(not_found))
        .route("/api/{*path}", get(not_found).post(not_found))
        .method_not_allowed_fallback(not_found)
        .fallback_service(
            ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html"))),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(app.clone(), security))
        .with_state(app)
}
async fn security(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let api = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    // Defend local reads against DNS rebinding as well as cross-origin writes.
    let host_ok = request.headers().get_all(header::HOST).iter().count() <= 1
        && request
            .headers()
            .get(header::HOST)
            .is_none_or(|h| h.to_str().is_ok_and(local_host))
        && request
            .uri()
            .authority()
            .is_none_or(|a| local_host(a.as_str()));
    let mut response = if api && !host_ok {
        Error::new(
            StatusCode::FORBIDDEN,
            "host_rejected",
            "Only local loopback hosts are permitted.",
        )
        .into_response()
    } else if api && request.method() == http::Method::POST {
        let headers = request.headers();
        let header_ok = headers.get_all("x-pullboard-request").iter().count() == 1
            && headers.get("x-pullboard-request").is_some_and(|h| h == "1");
        let origin_ok = headers.get_all(header::ORIGIN).iter().count() <= 1
            && headers.get(header::ORIGIN).is_none_or(|o| {
                o.to_str().is_ok_and(|o| {
                    app.config
                        .server
                        .allowed_origins
                        .iter()
                        .any(|allowed| allowed == o)
                })
            });
        if !header_ok || !origin_ok {
            Error::new(
                StatusCode::FORBIDDEN,
                "request_rejected",
                "A same-origin PullBoard request header is required.",
            )
            .into_response()
        } else {
            next.run(request).await
        }
    } else {
        next.run(request).await
    };
    if api {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
    }
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ),
    );
    response
}
fn local_host(host: &str) -> bool {
    url::Url::parse(&format!("http://{host}")).is_ok_and(|u| {
        u.username().is_empty()
            && u.password().is_none()
            && u.host_str().is_some_and(|h| {
                h == "localhost"
                    || h.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
    })
}
async fn not_found() -> Error {
    Error::not_found()
}
async fn health(State(app): State<Arc<App>>) -> Json<Health> {
    let mode = app.mode.read();
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        read_only_test_mode: mode.read_only_test_mode,
        mode_revision: mode.mode_revision,
    })
}
async fn runtime_mode(State(app): State<Arc<App>>) -> Json<RuntimeMode> {
    Json(app.mode.read())
}
async fn change_runtime_mode(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<ChangeRuntimeModeRequest>,
) -> Result<Json<RuntimeMode>> {
    Ok(Json(app.mode.change(
        &r.expected_mode_revision,
        r.read_only_test_mode,
    )?))
}
async fn board(State(app): State<Arc<App>>) -> Result<Json<Board>> {
    Ok(Json(app.cache.read().await?.board))
}
async fn context(State(app): State<Arc<App>>) -> Result<Json<Context>> {
    let snapshot = app.cache.read().await?;
    let jira_projects = snapshot
        .jira_choices
        .as_ref()
        .map(|m| m.projects.clone())
        .unwrap_or_default();
    let current_project = app
        .config
        .jira
        .as_ref()
        .and_then(|j| j.projects.first())
        .and_then(|key| jira_projects.iter().find(|p| &p.key == key))
        .cloned();
    let mut current_sprints: Vec<_> = snapshot
        .board
        .sprints
        .into_iter()
        .filter(|s| s.state == "active" && s.default_eligible)
        .collect();
    current_sprints.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    let ai = &app.config.ai;
    let mode = app.mode.read();
    Ok(Json(Context {
        read_only_test_mode: mode.read_only_test_mode,
        mode_revision: mode.mode_revision,
        refresh_interval_seconds: app.config.refresh_interval_seconds,
        identities: snapshot.identities,
        current_project,
        current_sprints,
        jira_projects,
        ai: AiContext {
            enabled: ai.enabled,
            provider: ai.enabled.then(|| ai.provider.name().into()),
            model: ai.enabled.then(|| ai.model.clone()),
        },
    }))
}
async fn sync_status(State(app): State<Arc<App>>) -> Result<Json<SyncStatus>> {
    Ok(Json(app.sync.status().await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
async fn refresh(
    State(app): State<Arc<App>>,
    CheckedJson(_): CheckedJson<Empty>,
) -> Result<(StatusCode, Json<SyncStatus>)> {
    Ok((StatusCode::ACCEPTED, Json(app.sync.request().await?)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionsQuery {
    project_key: String,
    purpose: Purpose,
    issue_type: Option<String>,
    #[serde(default)]
    query: String,
}
async fn options(
    State(app): State<Arc<App>>,
    query: std::result::Result<Query<OptionsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<JiraOptions>> {
    let Query(q) = query.map_err(|_| bad_json())?;
    if q.project_key.is_empty() || q.issue_type.as_ref().is_some_and(|s| s.is_empty()) {
        return Err(bad_json());
    }
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    Ok(Json(
        jira.options(&q.project_key, q.purpose, q.issue_type.as_deref(), &q.query)
            .await?,
    ))
}
async fn draft(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<DraftRequest>,
) -> Result<Json<TicketDraft>> {
    let snapshot = app.cache.read().await?;
    let pr = snapshot
        .board
        .pull_requests
        .iter()
        .find(|pr| pr.id == r.pull_request_id)
        .ok_or_else(Error::not_found)?;
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    let key = r
        .project_key
        .as_deref()
        .or_else(|| {
            app.config
                .forges
                .iter()
                .find(|f| f.id == pr.source_id)
                .and_then(|f| f.default_project.as_deref())
        })
        .or_else(|| jira.config.projects.first().map(String::as_str))
        .ok_or_else(Error::unavailable)?;
    let options = jira.options(key, r.purpose, None, "").await?;
    let (proposal, generated, warning) = app.ai.propose(pr, r.purpose).await;
    Ok(Json(TicketDraft {
        pull_request_id: r.pull_request_id,
        purpose: r.purpose,
        project_key: key.into(),
        title: proposal.title,
        description: proposal.description,
        story_points: None,
        assignee_id: options.defaults.assignee_id,
        sprint_id: options.defaults.sprint_id,
        issue_type: options.defaults.issue_type,
        team: options.defaults.team,
        suggested_status: None,
        generated_by: if generated { "ai" } else { "template" }.into(),
        warning,
        diagnostics: options.diagnostics,
    }))
}
async fn create(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<CreateIssueRequest>,
) -> Result<Json<CreateIssueResult>> {
    let read_only = app.mode.admit(&r.expected_mode_revision)?;
    let snapshot = app.cache.read().await?;
    let pr = snapshot
        .board
        .pull_requests
        .iter()
        .find(|pr| pr.id == r.pull_request_id)
        .ok_or_else(Error::not_found)?;
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    if !read_only {
        check_jira_backoff(&snapshot.board)?;
    }
    let result = jira
        .create(&r, pr, &snapshot.board.sprints, read_only)
        .await?;
    if result.outcome == "created"
        && let Err(e) = app.sync.after_write().await
    {
        tracing::warn!(code = e.code, "created ticket; refresh scheduling failed");
    }
    Ok(Json(result))
}
async fn link(
    State(app): State<Arc<App>>,
    path: std::result::Result<Path<String>, axum::extract::rejection::PathRejection>,
    CheckedJson(r): CheckedJson<LinkRequest>,
) -> Result<Json<LinkResult>> {
    let Path(key) = path.map_err(|_| bad_json())?;
    let read_only = app.mode.admit(&r.expected_mode_revision)?;
    let snapshot = app.cache.read().await?;
    let issue = snapshot
        .board
        .issues
        .iter()
        .find(|i| i.key == key)
        .ok_or_else(Error::not_found)?;
    let pr = snapshot
        .board
        .pull_requests
        .iter()
        .find(|pr| pr.id == r.pull_request_id)
        .ok_or_else(Error::not_found)?;
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    jira.project(&issue.project_key)?;
    if !read_only {
        check_jira_backoff(&snapshot.board)?;
    }
    let result = jira
        .link(&key, pr, issue.linked_pr_ids.contains(&pr.id), read_only)
        .await?;
    if result.outcome == "linked"
        && let Err(e) = app.sync.after_write().await
    {
        tracing::warn!(code = e.code, "linked PR; refresh scheduling failed");
    }
    Ok(Json(result))
}

async fn resolve_upstream_issue(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<ResolveUpstreamIssueRequest>,
) -> Result<Json<ResolvedUpstreamIssue>> {
    let matches: Vec<_> = app
        .config
        .forges
        .iter()
        .filter(|f| r.source_id.as_ref().is_none_or(|id| id == &f.id))
        .filter_map(|f| upstream::parse_url(f, &r.url).ok().map(|route| (f, route)))
        .collect();
    if matches.len() > 1 {
        return Err(Error::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ambiguous_upstream_source",
            "Select a configured source explicitly for this URL.",
        ));
    }
    let (config, route) = matches
        .into_iter()
        .next()
        .ok_or_else(upstream::invalid_url)?;
    upstream::check_policy(config, &route)?;
    let snapshot = app.cache.read().await?;
    let identity = snapshot
        .identities
        .iter()
        .find(|i| i.source_id == config.id && i.kind == config.kind)
        .ok_or_else(upstream::not_ready)?;
    let scope = snapshot
        .source_accounts
        .get(&config.id)
        .ok_or_else(upstream::not_ready)?;
    check_source_backoff(&snapshot.board, &config.id)?;
    // Manual reads require a verified identity, not completed assigned discovery.
    let forge = app
        .forges
        .iter()
        .find(|f| f.source_id() == config.id)
        .ok_or_else(upstream::not_ready)?;
    let issue = forge.issue(&route, identity).await?;
    let warning = upstream::warning(&issue);
    // Scope admission and installation serialize with principal quarantine.
    app.cache.store_resolution(scope, issue.clone()).await?;
    Ok(Json(ResolvedUpstreamIssue { issue, warning }))
}
async fn selected_upstream_issue(
    app: &App,
    snapshot: &crate::cache::Snapshot,
    id: &str,
) -> Result<UpstreamIssue> {
    let issue = app.cache.resolutions.select(id, snapshot)?;
    let config = app
        .config
        .forges
        .iter()
        .find(|f| f.id == issue.source_id && f.kind == issue.forge)
        .ok_or_else(upstream::not_found)?;
    if !snapshot
        .identities
        .iter()
        .any(|i| i.source_id == config.id && i.kind == config.kind)
        || !snapshot.source_accounts.contains_key(&config.id)
    {
        return Err(upstream::not_ready());
    }
    let route = upstream::parse_url(config, &issue.url)?;
    upstream::check_policy(config, &route)?;
    if route.owner != issue.owner
        || route.repository != issue.repository
        || route.number != issue.number
    {
        return Err(upstream::not_found());
    }
    Ok(issue)
}
async fn draft_upstream_issue(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<UpstreamIssueDraftRequest>,
) -> Result<Json<UpstreamIssueDraft>> {
    if r.upstream_issue_id.is_empty() || r.project_key.as_ref().is_some_and(|s| s.is_empty()) {
        return Err(bad_json());
    }
    let snapshot = app.cache.read().await?;
    let issue = selected_upstream_issue(&app, &snapshot, &r.upstream_issue_id).await?;
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    let key = r
        .project_key
        .as_deref()
        .or_else(|| {
            app.config
                .forges
                .iter()
                .find(|f| f.id == issue.source_id)
                .and_then(|f| f.default_project.as_deref())
        })
        .or_else(|| jira.config.projects.first().map(String::as_str))
        .ok_or_else(Error::unavailable)?;
    let options = jira.options(key, Purpose::Implementation, None, "").await?;
    Ok(Json(UpstreamIssueDraft {
        upstream_issue_id: issue.id.clone(),
        purpose: ImplementationPurpose::Implementation,
        project_key: key.into(),
        title: issue.title.clone(),
        description: upstream::description(&issue),
        story_points: None,
        assignee_id: options.defaults.assignee_id,
        sprint_id: options.defaults.sprint_id,
        issue_type: options.defaults.issue_type,
        team: options.defaults.team,
        suggested_status: None,
        generated_by: "template".into(),
        warning: upstream::warning(&issue),
        diagnostics: options.diagnostics,
    }))
}
async fn create_upstream_issue(
    State(app): State<Arc<App>>,
    CheckedJson(r): CheckedJson<CreateUpstreamIssueRequest>,
) -> Result<Json<CreateIssueResult>> {
    let read_only = app.mode.admit(&r.expected_mode_revision)?;
    let snapshot = app.cache.read().await?;
    let issue = selected_upstream_issue(&app, &snapshot, &r.upstream_issue_id).await?;
    let jira = app.jira.as_ref().ok_or_else(Error::unavailable)?;
    if !read_only {
        check_jira_backoff(&snapshot.board)?;
    }
    let result = jira.create_upstream_issue(&r, &issue, read_only).await?;
    if result.outcome == "created"
        && let Err(e) = app.sync.after_write().await
    {
        tracing::warn!(code = e.code, "created ticket; refresh scheduling failed");
    }
    Ok(Json(result))
}
fn check_jira_backoff(board: &Board) -> Result<()> {
    check_source_backoff(board, "jira")
}
fn check_source_backoff(board: &Board, source_id: &str) -> Result<()> {
    if let Some(at) = board
        .sources
        .iter()
        .find(|s| s.source_id == source_id)
        .and_then(|s| s.retry_at)
        .filter(|at| *at > chrono::Utc::now())
    {
        return Err(Error::backoff(at));
    }
    Ok(())
}
fn bad_json() -> Error {
    Error::new(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "The JSON body or query parameters are malformed, missing fields or contain unknown fields.",
    )
}
pub struct CheckedJson<T>(pub T);
pub trait RequiredFields {
    const REQUIRED: &'static [&'static str];
}
impl RequiredFields for Empty {
    const REQUIRED: &'static [&'static str] = &[];
}
impl RequiredFields for DraftRequest {
    const REQUIRED: &'static [&'static str] = &["pull_request_id", "purpose", "project_key"];
}
impl RequiredFields for ResolveUpstreamIssueRequest {
    const REQUIRED: &'static [&'static str] = &["source_id", "url"];
}
impl RequiredFields for UpstreamIssueDraftRequest {
    const REQUIRED: &'static [&'static str] = &["upstream_issue_id", "project_key"];
}
impl RequiredFields for CreateUpstreamIssueRequest {
    const REQUIRED: &'static [&'static str] = &[
        "upstream_issue_id",
        "expected_mode_revision",
        "purpose",
        "project_key",
        "issue_type",
        "title",
        "description",
        "story_points",
        "assignee_id",
        "sprint_id",
        "team",
        "status",
    ];
}
impl RequiredFields for CreateIssueRequest {
    const REQUIRED: &'static [&'static str] = &[
        "pull_request_id",
        "expected_mode_revision",
        "purpose",
        "project_key",
        "issue_type",
        "title",
        "description",
        "story_points",
        "assignee_id",
        "sprint_id",
        "team",
        "status",
    ];
}
impl RequiredFields for LinkRequest {
    const REQUIRED: &'static [&'static str] = &["pull_request_id", "expected_mode_revision"];
}
impl RequiredFields for ChangeRuntimeModeRequest {
    const REQUIRED: &'static [&'static str] = &["read_only_test_mode", "expected_mode_revision"];
}
impl<S, T> FromRequest<S> for CheckedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + RequiredFields + Send,
{
    type Rejection = Error;
    async fn from_request(
        request: Request<Body>,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Json(value) = Json::<serde_json::Value>::from_request(request, state)
            .await
            .map_err(|_| bad_json())?;
        if !value.is_object() || T::REQUIRED.iter().any(|key| value.get(*key).is_none()) {
            return Err(bad_json());
        }
        if T::REQUIRED.contains(&"expected_mode_revision")
            && !value
                .get("expected_mode_revision")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.is_empty() && s.chars().count() <= 128)
        {
            return Err(bad_json());
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(|_| bad_json())
    }
}
