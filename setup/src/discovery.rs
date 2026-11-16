//! Setup-only GET discovery. Every paginated inventory is complete or fails closed.
use crate::prompt::Choice;
use anyhow::{Context, Result, bail, ensure};
use pullboard_core::{
    config::{AiConfig, AiProvider, ForgeConfig, JiraConfig, Upstream},
    request_budget::{Bucket, RequestBudget},
    transport::{ReadDiagnosticError, ReadHttp, ReadTransport, bounded_body, http_client},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    sync::Mutex,
};

const FAILED: &str =
    "discovery failed or was incomplete; check credentials, permissions and pagination bounds";
pub trait GetJson {
    fn get_json(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> impl Future<Output = Result<Value>> + Send;
    fn limits(&self) -> &Upstream;
}
impl GetJson for ReadHttp {
    async fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        self.get_diagnostic(path, params).await
    }
    fn limits(&self) -> &Upstream {
        self.limits()
    }
}

const CACHE_ENTRIES: usize = 128;
const CACHE_BYTES: usize = 16 * 1024 * 1024;
type CacheKey = (String, Vec<(String, String)>);

#[derive(Default)]
struct SuccessfulReads {
    entries: VecDeque<(CacheKey, Value, usize)>,
    bytes: usize,
}
impl SuccessfulReads {
    fn get(&self, key: &CacheKey) -> Option<Value> {
        self.entries
            .iter()
            .find(|(stored, _, _)| stored == key)
            .map(|(_, value, _)| value.clone())
    }
    fn insert(&mut self, key: CacheKey, value: Value) {
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return;
        };
        let size = bytes
            .len()
            .saturating_add(key.0.len())
            .saturating_add(key.1.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>());
        if size > CACHE_BYTES || self.entries.iter().any(|(old, _, _)| old == &key) {
            return;
        }
        while self.entries.len() >= CACHE_ENTRIES || self.bytes + size > CACHE_BYTES {
            let Some((_, _, removed)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= removed;
        }
        self.bytes += size;
        self.entries.push_back((key, value, size));
    }
}

/// One credential-scoped setup session. Drop this client whenever URL, email or
/// token changes. Only successful repeated reads and verified list rows are reused;
/// full inventory pages are not retained. The FIFO has 128 entries / 16 MiB of
/// serialized keys and JSON, in memory only. No lock spans an upstream request.
pub struct JiraClient {
    http: ReadHttp,
    successful: Mutex<SuccessfulReads>,
}
impl JiraClient {
    pub async fn new(jira: &JiraConfig, limits: &Upstream) -> Result<Self> {
        Ok(Self {
            http: ReadHttp::jira(jira, limits)
                .await
                .context("initialize Jira discovery client")?,
            successful: Mutex::default(),
        })
    }
    fn cache_key(path: &str, params: &[(&str, String)]) -> CacheKey {
        let mut params: Vec<_> = params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        params.sort();
        (path.to_owned(), params)
    }
    fn remember(&self, path: &str, params: &[(&str, String)], value: &Value) -> Result<()> {
        let mut cache = self
            .successful
            .lock()
            .map_err(|_| anyhow::anyhow!("Jira discovery cache is unavailable"))?;
        let direct = path.strip_prefix("rest/api/3/project/").is_some_and(|key| {
            pullboard_core::config::project_key(key)
                && project_choice(value).is_ok_and(|choice| choice.id == key)
        }) || path
            .strip_prefix("rest/agile/1.0/board/")
            .is_some_and(|id| {
                positive_id(id).is_ok() && board_choice(value).is_ok_and(|choice| choice.id == id)
            });
        // List rows already verify names/identities: do not hydrate each item.
        // These aliases are bounded too, and never imply that a recent list is complete.
        let projects = if path == "rest/api/3/project/recent" {
            value.as_array()
        } else if path == "rest/api/3/project/search" {
            value.get("values").and_then(Value::as_array)
        } else {
            None
        };
        let projects = projects.filter(|rows| project_choices(rows).is_ok());
        let boards = if path == "rest/greenhopper/1.0/rapidviews/recent" {
            value.get("views").and_then(Value::as_array)
        } else if path == "rest/agile/1.0/board"
            && params.iter().any(|(k, _)| *k == "projectKeyOrId")
        {
            value.get("values").and_then(Value::as_array)
        } else {
            None
        };
        let boards = boards.filter(|rows| {
            rows.iter()
                .map(board_choice)
                .collect::<Result<Vec<_>>>()
                .and_then(|choices| unique_choices(&choices))
                .is_ok()
        });
        let reusable = match path {
            "rest/api/3/myself" => {
                text(value, "accountId").is_ok()
                    && text(value, "displayName").is_ok()
                    && value.get("active").and_then(Value::as_bool) == Some(true)
                    && value.get("accountType").and_then(Value::as_str) != Some("app")
            }
            "rest/api/3/field" => value.as_array().is_some_and(|rows| {
                rows.iter()
                    .all(|row| text(row, "name").is_ok() && text(row, "id").is_ok())
            }),
            "rest/api/3/project/recent" => projects.is_some(),
            "rest/greenhopper/1.0/rapidviews/recent" => boards.is_some(),
            "rest/api/3/project/search" => {
                projects.is_some_and(|rows| complete_name_search(value, rows, params))
            }
            _ => direct,
        };
        if reusable {
            cache.insert(Self::cache_key(path, params), value.clone());
        }
        if let Some(rows) = projects {
            for row in rows {
                if let (Ok(key), Ok(_)) = (text(row, "key"), text(row, "name"))
                    && pullboard_core::config::project_key(key)
                {
                    cache.insert((format!("rest/api/3/project/{key}"), vec![]), row.clone());
                }
            }
        }
        if let Some(rows) = boards {
            for row in rows {
                if let (Ok(id), Ok(_)) = (numeric(row, "id"), text(row, "name")) {
                    cache.insert((format!("rest/agile/1.0/board/{id}"), vec![]), row.clone());
                }
            }
        }
        Ok(())
    }
}
fn complete_name_search(value: &Value, rows: &[Value], params: &[(&str, String)]) -> bool {
    let Some((_, query)) = params.iter().find(|(key, _)| *key == "query") else {
        return false;
    };
    let total = value.get("total").and_then(Value::as_u64);
    let last = value.get("isLast").and_then(Value::as_bool);
    let count = u64::try_from(rows.len()).ok();
    // Cache only successful, self-contained name lookups. Incomplete/malformed
    // pages and negative matches must be fetched again on an explicit retry.
    value.get("startAt").and_then(Value::as_u64) == Some(0)
        && params
            .iter()
            .any(|(key, value)| *key == "startAt" && value == "0")
        && value.get("total").is_none_or(|v| v.as_u64().is_some())
        && value.get("isLast").is_none_or(|v| v.as_bool().is_some())
        && last != Some(false)
        && (last == Some(true) || total == count)
        && total.is_none_or(|total| Some(total) == count)
        && rows
            .iter()
            .any(|row| row.get("name").and_then(Value::as_str) == Some(query.as_str()))
}

impl GetJson for JiraClient {
    async fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let cached = self
            .successful
            .lock()
            .map_err(|_| anyhow::anyhow!("Jira discovery cache is unavailable"))?
            .get(&Self::cache_key(path, params));
        if let Some(value) = cached {
            return Ok(value);
        }
        let value = self.http.get_diagnostic(path, params).await?;
        self.remember(path, params, &value)?;
        Ok(value)
    }
    fn limits(&self) -> &Upstream {
        self.http.limits()
    }
}

pub fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("Jira response is missing a nonempty {key} field"))
}
fn numeric(v: &Value, key: &str) -> Result<String> {
    let n = v
        .get(key)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .with_context(|| format!("Jira response has no positive numeric {key} field"))?;
    Ok(n.to_string())
}

/// Supports documented Jira values/issueTypes/fields pages, rejecting repeated rows/offsets.
pub async fn pages(
    client: &impl GetJson,
    path: &str,
    params: &[(&str, String)],
    keys: &[&str],
) -> Result<Vec<Value>> {
    collect_pages(client, path, params, keys)
        .await
        .with_context(|| format!("complete {}", inventory(path)))
}

async fn collect_pages(
    client: &impl GetJson,
    path: &str,
    params: &[(&str, String)],
    keys: &[&str],
) -> Result<Vec<Value>> {
    let mut start = 0u64;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut expected_total = None;
    for _ in 0..client.limits().max_pages {
        let mut query = params.to_vec();
        query.extend([
            ("startAt", start.to_string()),
            ("maxResults", client.limits().page_size.to_string()),
        ]);
        let page = client
            .get_json(path, &query)
            .await
            .with_context(|| format!("read {} page at offset {start}", inventory(path)))?;
        ensure!(
            page.get("startAt").and_then(Value::as_u64) == Some(start),
            "Jira pagination returned a missing, invalid or unexpected startAt (expected {start})"
        );
        let values = keys
            .iter()
            .find_map(|k| page.get(*k).and_then(Value::as_array))
            .context("Jira pagination response is missing its expected array")?;
        for value in values {
            ensure!(
                seen.insert(value.to_string()),
                "Jira pagination repeated a row; inventory is incomplete"
            );
        }
        let next = start
            .checked_add(u64::try_from(values.len())?)
            .context("Jira pagination offset overflow")?;
        ensure!(
            page.get("total").is_none_or(|v| v.as_u64().is_some())
                && page.get("isLast").is_none_or(|v| v.as_bool().is_some()),
            "Jira pagination returned an invalid total or isLast"
        );
        let total = page.get("total").and_then(Value::as_u64);
        if let Some(total) = total {
            ensure!(
                expected_total.is_none_or(|old| old == total),
                "Jira pagination total changed; inventory is incomplete"
            );
            expected_total = Some(total);
        }
        if let Some(total) = expected_total {
            ensure!(
                next <= total,
                "Jira pagination returned more rows than its total"
            );
        }
        result.extend(values.iter().cloned());
        let last = page.get("isLast").and_then(Value::as_bool);
        if last == Some(true) || expected_total == Some(next) {
            ensure!(
                last != Some(false) || expected_total.is_none(),
                "Jira pagination reports more pages after its total"
            );
            if last == Some(true) {
                ensure!(
                    expected_total.is_none_or(|t| t == next),
                    "Jira pagination ended before its promised total; inventory is incomplete"
                );
            }
            return Ok(result);
        }
        ensure!(
            !values.is_empty(),
            "Jira pagination returned an empty page without completing its inventory"
        );
        start = next;
    }
    bail!(
        "{} exceeded the configured pagination limit of {} pages; inventory is incomplete",
        inventory(path),
        client.limits().max_pages
    )
}

fn inventory(path: &str) -> &'static str {
    match path {
        "rest/api/3/project/search" => "Jira project inventory",
        "rest/agile/1.0/board" => "Jira selected-project board inventory",
        "rest/api/3/group/bulk" => "Jira group inventory",
        "rest/api/3/group/member" => "Jira group membership inventory",
        _ if path.ends_with("/sprint") => "Jira active/future sprint inventory",
        _ if path.contains("/createmeta/") => "Jira create metadata inventory",
        _ => "Jira inventory",
    }
}

#[derive(Clone, Default)]
pub struct Catalog {
    pub identity: String,
    pub projects: Vec<Choice>,
    pub groups: Vec<Choice>,
    pub fields: Vec<Choice>,
}
pub async fn catalog(jira: &JiraConfig, limits: &Upstream) -> Result<Catalog> {
    let http = JiraClient::new(jira, limits).await?;
    catalog_from(&http).await
}
pub async fn catalog_from(http: &impl GetJson) -> Result<Catalog> {
    let me = http
        .get_json("rest/api/3/myself", &[])
        .await
        .context("verify Jira identity")?;
    text(&me, "accountId")?;
    ensure!(
        me.get("active").and_then(Value::as_bool) == Some(true)
            && me.get("accountType").and_then(Value::as_str) != Some("app"),
        "Jira identity is not an active human account"
    );
    let recent = http
        .get_json("rest/api/3/project/recent", &[])
        .await
        .context("load recent Jira projects (non-exhaustive); manual lookup remains available")?;
    let projects = project_choices(
        recent
            .as_array()
            .context("recent Jira projects response is not an array")?,
    )?;
    Ok(Catalog {
        identity: text(&me, "displayName")?.into(),
        projects,
        ..Catalog::default()
    })
}

/// Field discovery is deliberately separate from initial project/board selection.
pub async fn settings_from(http: &impl GetJson) -> Result<Catalog> {
    let fields = http
        .get_json("rest/api/3/field", &[])
        .await
        .context("load Jira field inventory")?;
    let fields = fields
        .as_array()
        .context("Jira field inventory response is not an array")?
        .iter()
        .map(|f| Ok(Choice::new(text(f, "name")?, text(f, "name")?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(Catalog {
        fields,
        ..Catalog::default()
    })
}

fn group_choices(rows: &[Value]) -> Result<Vec<Choice>> {
    let groups = rows
        .iter()
        .map(|g| Ok(Choice::new(text(g, "name")?, text(g, "name")?)))
        .collect::<Result<Vec<_>>>()?;
    unique_choices(&groups)?;
    Ok(groups)
}

/// This documented endpoint returns an array, not a paginated tenant inventory.
pub async fn own_groups_from(http: &impl GetJson) -> Result<Vec<Choice>> {
    let me = http
        .get_json("rest/api/3/myself", &[])
        .await
        .context("verify Jira identity for group discovery")?;
    let account = text(&me, "accountId")?;
    ensure!(
        me.get("active").and_then(Value::as_bool) == Some(true)
            && me.get("accountType").and_then(Value::as_str) != Some("app"),
        "Jira identity is not an active human account"
    );
    let groups = http
        .get_json("rest/api/3/user/groups", &[("accountId", account.into())])
        .await
        .context("load your Jira group memberships")?;
    group_choices(
        groups
            .as_array()
            .context("Jira user groups response is not an array")?,
    )
}

/// Only called after an explicitly confirmed full scan.
pub async fn all_groups_from(http: &impl GetJson) -> Result<Vec<Choice>> {
    group_choices(&pages(http, "rest/api/3/group/bulk", &[], &["values"]).await?)
}

pub async fn group_from(http: &impl GetJson, name: &str) -> Result<Choice> {
    ensure!(!name.trim().is_empty(), "enter an exact group name");
    let rows = pages(
        http,
        "rest/api/3/group/bulk",
        &[("groupName", name.into())],
        &["values"],
    )
    .await?;
    let choices = group_choices(&rows)?;
    ensure!(
        choices.len() == 1 && choices[0].id == name,
        "no unique exact Jira group match was verified"
    );
    verify_group_from(http, name).await?;
    choices.into_iter().next().context("missing verified group")
}

async fn verify_group_from(http: &impl GetJson, name: &str) -> Result<()> {
    pages(
        http,
        "rest/api/3/group/member",
        &[
            ("groupname", name.into()),
            ("includeInactiveUsers", "false".into()),
        ],
        &["values"],
    )
    .await?;
    Ok(())
}
fn unique_choices(choices: &[Choice]) -> Result<()> {
    let mut seen = BTreeSet::new();
    ensure!(
        choices.iter().all(|c| seen.insert(&c.id)),
        "discovery returned ambiguous choice identities"
    );
    Ok(())
}

/// Recent boards only. An unsupported internal endpoint is an error, never an
/// implicit full or global board scan; callers can offer manual lookup instead.
pub async fn boards(jira: &JiraConfig, limits: &Upstream) -> Result<Vec<Choice>> {
    let http = JiraClient::new(jira, limits).await?;
    boards_from(&http).await
}
pub async fn boards_from(http: &impl GetJson) -> Result<Vec<Choice>> {
    let recent = http.get_json("rest/greenhopper/1.0/rapidviews/recent", &[]).await.context("load recent Jira boards (non-exhaustive; internal endpoint); manual lookup remains available")?;
    let choices = recent
        .get("views")
        .and_then(Value::as_array)
        .context("recent Jira boards response is missing its views array")?
        .iter()
        .map(board_choice)
        .collect::<Result<Vec<_>>>()?;
    unique_choices(&choices)?;
    Ok(choices)
}

pub async fn all_projects_from(http: &impl GetJson) -> Result<Vec<Choice>> {
    project_choices(&pages(http, "rest/api/3/project/search", &[], &["values"]).await?)
        .context("validate complete Jira project inventory")
}

fn project_choice(project: &Value) -> Result<Choice> {
    let key = text(project, "key")?;
    ensure!(
        pullboard_core::config::project_key(key),
        "Jira returned an invalid project key"
    );
    Ok(Choice::new(
        key,
        format!("{key}  {}", text(project, "name")?),
    ))
}
fn project_choices(projects: &[Value]) -> Result<Vec<Choice>> {
    let choices = projects
        .iter()
        .map(project_choice)
        .collect::<Result<Vec<_>>>()?;
    unique_choices(&choices)?;
    Ok(choices)
}
fn board_choice(board: &Value) -> Result<Choice> {
    let id = numeric(board, "id")?;
    Ok(Choice::new(&id, format!("{id}  {}", text(board, "name")?)))
}

/// Explicit, complete board inventory for selected projects only. Shared boards
/// are deduplicated in first-project order. There is no global-board fallback.
pub async fn project_boards_from(http: &impl GetJson, jira: &JiraConfig) -> Result<Vec<Choice>> {
    ensure!(
        jira.projects
            .iter()
            .all(|key| pullboard_core::config::project_key(key)),
        "selected Jira project key is invalid"
    );
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for project in &jira.projects {
        for b in pages(
            http,
            "rest/agile/1.0/board",
            &[("projectKeyOrId", project.clone())],
            &["values"],
        )
        .await?
        {
            let choice =
                board_choice(&b).context("validate selected-project Jira board inventory")?;
            if seen.insert(choice.id.clone()) {
                out.push(choice);
            }
        }
    }
    Ok(out)
}

fn positive_id(input: &str) -> Result<u64> {
    ensure!(
        !input.is_empty() && input.bytes().all(|c| c.is_ascii_digit()),
        "Jira board ID must be a positive integer"
    );
    let id = input.parse::<u64>().context("Jira board ID is too large")?;
    ensure!(id > 0, "Jira board ID must be a positive integer");
    Ok(id)
}

struct BrowserRoute {
    project: Option<String>,
    board: Option<u64>,
}

/// Parse only known Jira browser routes. Never fetch the supplied browser URL:
/// all callers construct GET paths from the validated key or numeric ID instead.
fn browser_route(jira: &JiraConfig, input: &str) -> Result<BrowserRoute> {
    const INVALID: &str =
        "invalid Jira URL; use a standard project or board URL on the configured tenant";
    ensure!(
        input.len() <= 4096
            && !input.chars().any(|c| c.is_whitespace() || c.is_control())
            && !input.contains(['\\', '%']),
        "Jira URL contains an unsafe encoded path, separator or whitespace"
    );
    // URL parsers normalize dot segments, so reject them before parsing.
    let (_, raw) = input.split_once("://").context(INVALID)?;
    let raw_path = raw.find('/').map(|start| &raw[start..]).unwrap_or("");
    let raw_path = raw_path.split(['?', '#']).next().unwrap_or("");
    ensure!(
        !raw_path.split('/').any(|part| matches!(part, "." | "..")) && !raw_path.contains("//"),
        "Jira URL contains an unsafe or ambiguous path"
    );
    let url = url::Url::parse(input).map_err(|_| anyhow::anyhow!(INVALID))?;
    let base = url::Url::parse(&jira.base_url)
        .map_err(|_| anyhow::anyhow!("configured Jira tenant URL is invalid"))?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.origin() == base.origin()
            && url.host_str().is_some(),
        "Jira URL does not match the configured tenant origin (scheme, host and port)"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && !raw.split('/').next().unwrap_or("").contains('@')
            && url.fragment().is_none(),
        "Jira URL must not contain credentials or a fragment"
    );
    let base_path = base.path().trim_end_matches('/');
    let relative = url
        .path()
        .strip_prefix(base_path)
        .context("Jira URL is outside the configured tenant base path")?;
    ensure!(
        relative.starts_with('/'),
        "Jira URL is outside the configured tenant base path"
    );
    let segments: Vec<_> = relative.trim_matches('/').split('/').collect();
    let mut route = match segments.as_slice() {
        ["projects", key]
        | ["projects", key, "summary"]
        | ["browse", key]
        | ["jira", "projects", key]
        | ["jira", "software", "projects", key]
        | ["jira", "core", "projects", key]
        | ["jira", "servicedesk", "projects", key]
        | ["jira", "software", "c", "projects", key]
        | ["plugins", "servlet", "project-config", key]
        | ["plugins", "servlet", "project-config", key, "details"] => BrowserRoute {
            project: Some((*key).into()),
            board: None,
        },
        ["projects", key, view]
        | ["jira", "projects", key, view]
        | ["jira", "software", "projects", key, view]
        | ["jira", "core", "projects", key, view]
        | ["jira", "software", "c", "projects", key, view]
            if matches!(
                *view,
                "summary" | "issues" | "list" | "timeline" | "calendar" | "board"
            ) =>
        {
            BrowserRoute {
                project: Some((*key).into()),
                board: None,
            }
        }
        ["jira", "servicedesk", "projects", key, "queues"] => BrowserRoute {
            project: Some((*key).into()),
            board: None,
        },
        ["boards", id] | ["jira", "boards", id] => BrowserRoute {
            project: None,
            board: Some(positive_id(id)?),
        },
        ["jira", "software", "c", "projects", key, "boards", id]
        | ["jira", "software", "projects", key, "boards", id] => BrowserRoute {
            project: Some((*key).into()),
            board: Some(positive_id(id)?),
        },
        ["jira", "software", "c", "projects", key, "boards", id, view]
        | ["jira", "software", "projects", key, "boards", id, view]
            if matches!(*view, "backlog" | "active-sprints") =>
        {
            BrowserRoute {
                project: Some((*key).into()),
                board: Some(positive_id(id)?),
            }
        }
        ["secure", "RapidBoard.jspa"] => BrowserRoute {
            project: None,
            board: None,
        },
        _ => bail!(INVALID),
    };
    ensure!(
        route
            .project
            .as_deref()
            .is_none_or(pullboard_core::config::project_key),
        "Jira project URL contains an invalid project key"
    );
    let rapid = segments.as_slice() == ["secure", "RapidBoard.jspa"];
    let mut seen = BTreeSet::new();
    for (key, value) in url.query_pairs() {
        ensure!(
            seen.insert(key.to_string()),
            "Jira URL contains duplicate or ambiguous query parameters"
        );
        match key.as_ref() {
            "rapidView" if rapid => route.board = Some(positive_id(&value)?),
            "projectKey" if rapid && pullboard_core::config::project_key(&value) => {
                route.project = Some(value.into_owned());
            }
            "selectedSprint" if rapid || route.board.is_some() => {
                positive_id(&value)?;
            }
            "view"
                if rapid
                    && matches!(
                        value.as_ref(),
                        "planning" | "planning.nodetail" | "detail" | "reporting"
                    ) => {}
            _ => bail!("Jira URL contains an unsupported or ambiguous query parameter"),
        }
    }
    ensure!(
        !rapid || route.board.is_some(),
        "Jira RapidBoard URL is missing its positive rapidView ID"
    );
    Ok(route)
}

fn is_url_input(input: &str) -> bool {
    if input.contains("://") || input.starts_with(['/', '\\']) {
        return true;
    }
    if let Some((scheme, rest)) = input.split_once(':')
        && !scheme.is_empty()
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
        && (matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
            || !rest.starts_with(char::is_whitespace))
    {
        return true;
    }
    // Reject browser URLs missing their scheme, without mistaking ordinary
    // project names such as "Platform / API: v2" or "Growth 100%" for URLs.
    input
        .split_once('/')
        .is_some_and(|(host, _)| host.contains('.') && !host.chars().any(char::is_whitespace))
        || ["http%", "https%", "http/", "https/", "http\\", "https\\"]
            .iter()
            .any(|prefix| input.to_ascii_lowercase().starts_with(prefix))
}

fn not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<ReadDiagnosticError>()
            .is_some_and(|error| error.status == Some(http::StatusCode::NOT_FOUND))
            || cause
                .downcast_ref::<pullboard_core::error::Error>()
                .is_some_and(|error| {
                    error.status == http::StatusCode::NOT_FOUND
                        || error.code == "upstream_not_found"
                })
    })
}

/// Verify a submitted key, exact name or same-tenant project URL. All exact-name
/// matches are returned so that the prompt must explicitly resolve ambiguity.
pub async fn project_from(
    http: &impl GetJson,
    jira: &JiraConfig,
    input: &str,
) -> Result<Vec<Choice>> {
    ensure!(
        !input.chars().any(char::is_control),
        "Jira project input must not contain control characters"
    );
    let input = input.trim();
    ensure!(
        !input.is_empty() && input.len() <= 4096 && !input.chars().any(char::is_control),
        "enter a Jira project key, exact name or project URL"
    );
    let url_input = is_url_input(input);
    let key = if url_input {
        Some(
            browser_route(jira, input)?
                .project
                .context("Jira URL does not identify a project")?,
        )
    } else if pullboard_core::config::project_key(input) {
        Some(input.into())
    } else {
        None
    };
    if let Some(key) = key {
        match http
            .get_json(&format!("rest/api/3/project/{key}"), &[])
            .await
        {
            Ok(project) => {
                let choice = project_choice(&project)?;
                ensure!(
                    choice.id == key,
                    "Jira project lookup returned a different project key"
                );
                return Ok(vec![choice]);
            }
            // A project name can also look like a key. Only a genuine 404 allows
            // a filtered name search; permission/network errors never trigger scans.
            Err(error) if !url_input && not_found(&error) => {}
            Err(error) => return Err(error.context("verify submitted Jira project")),
        }
    }
    let rows = pages(
        http,
        "rest/api/3/project/search",
        &[("query", input.into())],
        &["values"],
    )
    .await
    .context("find submitted Jira project by exact name")?;
    project_choices(&rows)?;
    let matches: Vec<_> = rows
        .into_iter()
        .filter(|project| project.get("name").and_then(Value::as_str) == Some(input))
        .collect();
    ensure!(
        !matches.is_empty(),
        "no accessible Jira project has that exact name; check the name and Browse Projects permission"
    );
    project_choices(&matches)
}

/// Verify one manually submitted ID or URL, checking the returned board identity.
pub async fn board_from(http: &impl GetJson, jira: &JiraConfig, input: &str) -> Result<Choice> {
    ensure!(
        input.len() <= 4096 && !input.chars().any(char::is_control),
        "Jira board input is too long or contains control characters"
    );
    let input = input.trim();
    let id = if is_url_input(input) {
        browser_route(jira, input)?
            .board
            .context("Jira URL does not identify a board")?
    } else {
        positive_id(input)?
    };
    let board = http
        .get_json(&format!("rest/agile/1.0/board/{id}"), &[])
        .await
        .context("verify submitted Jira board")?;
    ensure!(
        board.get("id").and_then(Value::as_u64) == Some(id),
        "Jira board lookup returned a different board ID"
    );
    board_choice(&board)
}

#[derive(Clone)]
pub struct Sprint {
    pub id: String,
    /// First configured board returning this sprint, never originBoardId.
    pub board_id: u64,
    pub name: String,
    pub state: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeamOption {
    pub id: String,
    pub value: String,
}
#[derive(Clone)]
pub struct TypeMetadata {
    pub project: String,
    pub name: String,
    pub teams: Vec<String>,
    pub team_options: Vec<TeamOption>,
    pub statuses: Vec<String>,
    pub notes: Vec<String>,
}
#[derive(Clone, Default)]
pub struct Metadata {
    pub board_ids: Vec<u64>,
    pub team_field_name: String,
    pub team_field_id: Option<String>,
    pub types: Vec<TypeMetadata>,
    pub sprints: Vec<Sprint>,
}
pub async fn metadata(jira: &JiraConfig, limits: &Upstream) -> Result<Metadata> {
    let http = JiraClient::new(jira, limits).await?;
    metadata_from(&http, jira).await
}
pub async fn metadata_from(http: &impl GetJson, jira: &JiraConfig) -> Result<Metadata> {
    // A visible group is not proof of membership-read permission: verify every chosen pool.
    ensure!(
        !jira.assignee_groups.is_empty(),
        "select at least one assignee group"
    );
    for name in &jira.assignee_groups {
        verify_group_from(http, name).await?;
    }
    let fields = http
        .get_json("rest/api/3/field", &[])
        .await
        .context("load Jira fields for create metadata")?;
    let team_fields: Vec<_> = fields
        .as_array()
        .context("Jira field inventory response is not an array")?
        .iter()
        .filter(|f| f.get("name").and_then(Value::as_str) == Some(&jira.team_field_name))
        .collect();
    ensure!(
        team_fields.len() <= 1,
        "team field name is ambiguous; select a unique semantic field"
    );
    let team_id = team_fields.first().map(|f| text(f, "id")).transpose()?;
    if let Some(id) = team_id {
        validate_team_field_id(id)?;
    }
    pullboard_core::config::validate_board_ids(&jira.board_ids)?;
    let mut out = Metadata {
        board_ids: jira.board_ids.clone(),
        team_field_name: jira.team_field_name.clone(),
        team_field_id: team_id.map(str::to_owned),
        ..Metadata::default()
    };
    let mut sprint_ids = BTreeSet::new();
    // Sequential reads share the existing transport and pagination bounds. Build
    // the entire union privately; a required board/page failure returns no metadata.
    for &board_id in &jira.board_ids {
        let board = http
            .get_json(&format!("rest/agile/1.0/board/{board_id}"), &[])
            .await
            .context("verify selected Jira board for metadata")?;
        ensure!(
            board.get("id").and_then(Value::as_u64) == Some(board_id),
            "Jira metadata board lookup returned a different board ID"
        );
        let mut board_sprint_ids = BTreeSet::new();
        for s in pages(
            http,
            &format!("rest/agile/1.0/board/{board_id}/sprint"),
            &[("state", "active,future".into())],
            &["values"],
        )
        .await?
        {
            let id = numeric(&s, "id")?;
            ensure!(
                board_sprint_ids.insert(id.clone()),
                "Jira sprint inventory repeated an identity"
            );
            let state = text(&s, "state")?;
            ensure!(
                matches!(state, "active" | "future"),
                "Jira active/future sprint inventory returned an unexpected state"
            );
            if !sprint_ids.insert(id.clone()) {
                continue;
            }
            out.sprints.push(Sprint {
                id,
                board_id,
                name: text(&s, "name")?.into(),
                state: state.into(),
            });
        }
    }
    for project in &jira.projects {
        ensure!(
            pullboard_core::config::project_key(project),
            "selected Jira project key is invalid"
        );
        let types = pages(
            http,
            &format!("rest/api/3/issue/createmeta/{project}/issuetypes"),
            &[],
            &["issueTypes", "values"],
        )
        .await?;
        let groups = http
            .get_json(&format!("rest/api/3/project/{project}/statuses"), &[])
            .await
            .context("load Jira project status inventory")?;
        let groups = groups
            .as_array()
            .context("Jira project status inventory response is not an array")?;
        let mut names = BTreeSet::new();
        for t in types {
            let id = text(&t, "id")?;
            ensure!(
                id.bytes().all(|c| c.is_ascii_digit()),
                "Jira create metadata returned a nonnumeric issue-type ID"
            );
            let name = text(&t, "name")?;
            ensure!(names.insert(name.to_owned()), "ambiguous issue type names");
            let fields = pages(
                http,
                &format!("rest/api/3/issue/createmeta/{project}/issuetypes/{id}"),
                &[],
                &["fields", "values"],
            )
            .await?;
            let mut item = TypeMetadata {
                project: project.clone(),
                name: name.into(),
                teams: vec![],
                team_options: vec![],
                statuses: vec![],
                notes: vec![],
            };
            let matching: Vec<_> = groups
                .iter()
                .filter(|g| g.get("id").and_then(Value::as_str) == Some(id))
                .collect();
            ensure!(
                matching.len() == 1,
                "Jira status inventory has missing or ambiguous issue-type identities"
            );
            ensure!(
                text(matching[0], "name")? == name,
                "Jira status inventory and create metadata disagree on the issue-type name"
            );
            let mut status_names = BTreeMap::<String, usize>::new();
            for s in matching[0]
                .get("statuses")
                .and_then(Value::as_array)
                .context("Jira issue type is missing its statuses array")?
            {
                text(s, "id")?;
                *status_names.entry(text(s, "name")?.into()).or_default() += 1;
            }
            if status_names.values().any(|count| *count > 1) {
                item.notes
                    .push("ambiguous status names are unavailable as suggestions".into());
            }
            item.statuses = status_names
                .into_iter()
                .filter_map(|(n, count)| (count == 1).then_some(n))
                .collect();
            let mut field_ids = BTreeSet::new();
            for f in fields {
                let field_id = f
                    .get("fieldId")
                    .or_else(|| f.get("key"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .context("Jira create metadata is missing a field identity")?;
                ensure!(
                    field_ids.insert(field_id.to_owned()),
                    "ambiguous duplicate create fields"
                );
                if team_id.is_some()
                    && f.get("fieldId")
                        .or_else(|| f.get("key"))
                        .and_then(Value::as_str)
                        == team_id
                {
                    let writable = f
                        .get("operations")
                        .and_then(Value::as_array)
                        .is_some_and(|ops| ops.iter().any(|op| op.as_str() == Some("set")));
                    if writable
                        && f.pointer("/schema/type").and_then(Value::as_str) == Some("option")
                        && f.pointer("/schema/custom").and_then(Value::as_str)
                            == Some("com.atlassian.jira.plugin.system.customfieldtypes:select")
                    {
                        let mut option_ids = BTreeSet::new();
                        for v in f
                            .get("allowedValues")
                            .and_then(Value::as_array)
                            .context("Jira team select field is missing its allowedValues array")?
                        {
                            ensure!(
                                v.get("disabled").is_none_or(|v| v.as_bool().is_some()),
                                "Jira team option has an invalid disabled flag"
                            );
                            let id = team_text(v, "id")?;
                            let value = team_text(v, "value")?;
                            ensure!(
                                option_ids.insert(id.to_owned()),
                                "Jira team metadata repeated an option identity"
                            );
                            if v.get("disabled").and_then(Value::as_bool) != Some(true) {
                                item.teams.push(value.into());
                                item.team_options.push(TeamOption {
                                    id: id.into(),
                                    value: value.into(),
                                });
                            }
                        }
                    } else {
                        item.notes.push("team field has no supported select-option discovery; no UUIDs inferred".into());
                    }
                }
            }
            let mut team_counts = BTreeMap::<String, usize>::new();
            for team in &item.teams {
                *team_counts.entry(team.clone()).or_default() += 1;
            }
            if team_counts.values().any(|count| *count > 1) {
                item.notes
                    .push("ambiguous team value names are unavailable".into());
            }
            item.teams = team_counts
                .into_iter()
                .filter_map(|(name, count)| (count == 1).then_some(name))
                .collect();
            item.team_options
                .retain(|option| item.teams.contains(&option.value));
            if item.teams.is_empty() {
                item.notes
                    .push("no verified team choices for this project/type".into());
            }
            out.types.push(item);
        }
    }
    Ok(out)
}

fn validate_team_field_id(id: &str) -> Result<()> {
    ensure!(
        id.strip_prefix("customfield_")
            .is_some_and(|number| positive_id(number).is_ok()),
        "Jira team select field has an invalid field identity"
    );
    Ok(())
}

fn team_text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let value = text(value, key)?;
    ensure!(
        value.len() <= 4096 && crate::prompt::clean(value) == value,
        "Jira team option contains an unsafe or overlong value"
    );
    Ok(value)
}

#[derive(Clone, Default)]
pub struct BoardTeams {
    /// Semantic YAML names, each proven by an observed option ID/value pair.
    pub teams: Vec<String>,
    /// Well-formed issue options outside the selected projects' create choices.
    pub unverified_values: usize,
}

/// Read only the selected board's issues in ALL its returned active sprints.
/// Metadata is reused, never rediscovered; no regex, future/history or broader scan.
pub async fn board_teams_from(
    http: &impl GetJson,
    jira: &JiraConfig,
    metadata: &Metadata,
) -> Result<BoardTeams> {
    ensure!(
        !jira.board_ids.is_empty()
            && metadata.board_ids == jira.board_ids
            && metadata.team_field_name == jira.team_field_name,
        "team metadata does not match the selected board/field scope"
    );
    let field = metadata
        .team_field_id
        .as_deref()
        .context("selected team field has no verified field identity")?;
    validate_team_field_id(field)?;
    ensure!(
        metadata
            .types
            .iter()
            .all(|t| jira.projects.contains(&t.project)),
        "team metadata is outside the selected project scope"
    );
    let verified: BTreeSet<_> = metadata
        .types
        .iter()
        .flat_map(|t| t.team_options.iter())
        .map(|option| (option.id.as_str(), option.value.as_str()))
        .collect();
    let mut teams = BTreeSet::new();
    let mut unverified_values = BTreeSet::new();
    let mut sprint_ids = BTreeSet::new();
    for sprint in metadata
        .sprints
        .iter()
        .filter(|s| s.state == "active" && Some(&s.board_id) == jira.board_ids.first())
    {
        let id = positive_id(&sprint.id).context("invalid active sprint identity")?;
        ensure!(
            sprint_ids.insert(id),
            "active sprint inventory repeated an identity"
        );
        let path = format!(
            "rest/software/1.0/board/{}/sprint/{id}/issue",
            sprint.board_id
        );
        let mut next_token: Option<String> = None;
        let mut tokens = BTreeSet::new();
        let mut issues = BTreeSet::new();
        let mut complete = false;
        for _ in 0..http.limits().max_pages {
            let mut params = vec![
                ("fields", field.to_owned()),
                ("maxResults", http.limits().page_size.to_string()),
            ];
            if let Some(token) = &next_token {
                params.push(("nextPageToken", token.clone()));
            }
            let page = http.get_json(&path, &params).await.context(
                "read selected-board active-sprint issues; inventory is incomplete on failure",
            )?;
            let rows = page
                .get("issues")
                .and_then(Value::as_array)
                .context("Jira sprint issue page is missing its issues array")?;
            let last = page
                .get("isLast")
                .and_then(Value::as_bool)
                .context("Jira sprint issue page is missing a valid isLast flag")?;
            let token = page.get("nextPageToken");
            for row in rows {
                ensure!(
                    issues.insert(team_text(row, "id")?.to_owned()),
                    "Jira sprint issue pagination repeated an issue identity; inventory is incomplete"
                );
                let value = row
                    .get("fields")
                    .and_then(|fields| fields.get(field))
                    .context("Jira sprint issue is missing the requested team field")?;
                if value.is_null() {
                    continue;
                }
                let id = team_text(value, "id")?;
                let value = team_text(value, "value")?;
                if verified.contains(&(id, value)) {
                    teams.insert(value.to_owned());
                } else {
                    unverified_values.insert((id.to_owned(), value.to_owned()));
                }
            }
            if last {
                ensure!(
                    token.is_none_or(Value::is_null),
                    "Jira sprint issue pagination returned a token after its last page"
                );
                complete = true;
                break;
            }
            let token = token
                .and_then(Value::as_str)
                .filter(|token| {
                    !token.trim().is_empty()
                        && token.len() <= 4096
                        && !token.chars().any(char::is_control)
                })
                .context("Jira sprint issue pagination has no valid continuation token")?;
            ensure!(
                !rows.is_empty() && tokens.insert(token.to_owned()),
                "Jira sprint issue pagination repeated a token or returned an incomplete empty page"
            );
            next_token = Some(token.to_owned());
        }
        ensure!(
            complete,
            "Jira active-sprint issue inventory exceeded the configured pagination limit of {} pages; inventory is incomplete",
            http.limits().max_pages
        );
    }
    Ok(BoardTeams {
        teams: teams.into_iter().collect(),
        unverified_values: unverified_values.len(),
    })
}

pub async fn forge_identity(forge: &ForgeConfig, limits: &Upstream) -> Result<String> {
    let http = ReadTransport::new(forge, limits)
        .await
        .map_err(|_| anyhow::anyhow!(FAILED))?;
    let user: Value = http
        .get("user", &[])
        .await
        .map_err(|_| anyhow::anyhow!(FAILED))?;
    numeric(&user, "id")?;
    Ok(user
        .get("login")
        .or_else(|| user.get("username"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context(FAILED)?
        .into())
}

/// Explicitly confirmed synthetic inference only. No user/PR content, retries or tools.
pub async fn synthetic_ai_test(ai: &AiConfig, limits: &Upstream) -> Result<()> {
    ensure!(ai.enabled, "AI is disabled");
    let base = ai.base_url.as_deref().unwrap_or(match ai.provider {
        AiProvider::Openai => "https://api.openai.com/v1",
        AiProvider::Anthropic => "https://api.anthropic.com/v1",
        AiProvider::Ollama => "http://host.containers.internal:11434",
    });
    let base = url::Url::parse(&format!("{}/", base.trim_end_matches('/')))
        .context("invalid AI endpoint")?;
    let (path, body) = match ai.provider {
        AiProvider::Openai => (
            "chat/completions",
            json!({"model":ai.model,"messages":[{"role":"user","content":"Reply with the word ready. This is a synthetic PullBoard setup test."}],"max_completion_tokens":16}),
        ),
        AiProvider::Anthropic => (
            "messages",
            json!({"model":ai.model,"max_tokens":16,"messages":[{"role":"user","content":"Reply with ready. Synthetic PullBoard setup test."}]}),
        ),
        AiProvider::Ollama => (
            "api/generate",
            json!({"model":ai.model,"prompt":"Reply with ready. Synthetic PullBoard setup test.","stream":false,"options":{"num_predict":16}}),
        ),
    };
    let client = http_client(limits)?;
    let mut request = client.post(base.join(path)?).json(&body);
    if let Some(token) = &ai.token {
        let token = token
            .resolve()
            .await
            .map_err(|_| anyhow::anyhow!("reenter AI credential before testing"))?;
        let raw = match ai.provider {
            AiProvider::Anthropic => token.to_string(),
            _ => format!("Bearer {}", *token),
        };
        let mut header = http::HeaderValue::from_str(&raw).context("invalid AI credential")?;
        header.set_sensitive(true);
        request = request.header(
            if matches!(ai.provider, AiProvider::Anthropic) {
                "x-api-key"
            } else {
                "authorization"
            },
            header,
        );
    }
    if matches!(ai.provider, AiProvider::Anthropic) {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let budget = RequestBudget::shared(base.as_str(), limits)?;
    let _permit = budget.acquire(Bucket::General).await?;
    let response = request.send().await.map_err(|_| {
        anyhow::anyhow!("synthetic inference failed; editable template fallback remains available")
    })?;
    let status = response.status();
    let headers = response.headers().clone();
    let rate = budget.observe_headers(Bucket::General, status, &headers);
    let bytes = bounded_body(response, limits.max_response_bytes)
        .await
        .map_err(|_| {
            anyhow::anyhow!("synthetic inference response was unavailable or too large")
        })?;
    if let Some(error) = rate {
        return Err(error.into());
    }
    ensure!(
        status.is_success(),
        "synthetic inference rejected; check endpoint, exact model, credentials and billing; template fallback remains available"
    );
    let value: Value =
        serde_json::from_slice(&bytes).context("synthetic inference did not return JSON")?;
    let valid = match ai.provider {
        AiProvider::Openai => value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str),
        AiProvider::Anthropic => value.pointer("/content/0/text").and_then(Value::as_str),
        AiProvider::Ollama => value.get("response").and_then(Value::as_str),
    };
    ensure!(
        valid.is_some_and(|v| !v.is_empty()),
        "synthetic inference did not contain a completion; template fallback remains available"
    );
    Ok(())
}
