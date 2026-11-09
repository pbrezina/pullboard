//! Trusted issue routes, assignment-scoped reads and process-local manual selections.
use crate::{
    cache::Snapshot,
    config::ForgeConfig,
    error::{Error, Result},
    model::*,
    sync::Progress,
    transport::ReadTransport,
};
use futures_util::{StreamExt, TryStreamExt, stream};
use http::StatusCode;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRoute {
    pub owner: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}
pub fn invalid_url() -> Error {
    Error::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_upstream_url",
        "Use a canonical issue URL on the selected configured forge instance.",
    )
}
pub fn not_found() -> Error {
    Error::new(
        StatusCode::NOT_FOUND,
        "upstream_issue_not_found",
        "The upstream issue selection is unavailable; resolve the issue again.",
    )
}
pub fn not_ready() -> Error {
    Error::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "upstream_not_ready",
        "The forge identity has not been verified.",
    )
}
fn not_issue() -> Error {
    Error::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "not_upstream_issue",
        "Pull requests and merge requests are not upstream issues.",
    )
}
pub fn check_policy(config: &ForgeConfig, route: &IssueRoute) -> Result<()> {
    if !config.includes(&format!("{}/{}", route.owner, route.repository)) {
        return Err(Error::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "repository_not_allowed",
            "The repository is excluded by the configured source policy.",
        ));
    }
    Ok(())
}
pub fn web_base(config: &ForgeConfig) -> Result<url::Url> {
    let mut base = url::Url::parse(&config.base_url).map_err(|_| invalid_url())?;
    if config.kind == Kind::Github {
        if base.as_str().trim_end_matches('/') == "https://api.github.com" {
            return url::Url::parse("https://github.com/").map_err(|_| invalid_url());
        }
        if let Some(path) = base.path().trim_end_matches('/').strip_suffix("/api/v3") {
            let path = path.to_owned();
            base.set_path(&path);
        }
    }
    Ok(base)
}
fn decode_segment(raw: &str) -> Result<String> {
    let mut out = Vec::new();
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        out.push(if b == b'%' {
            let a = bytes
                .next()
                .and_then(|b| char::from(b).to_digit(16))
                .ok_or_else(invalid_url)?;
            let b = bytes
                .next()
                .and_then(|b| char::from(b).to_digit(16))
                .ok_or_else(invalid_url)?;
            u8::try_from(a * 16 + b).map_err(|_| invalid_url())?
        } else {
            b
        });
    }
    let text = String::from_utf8(out).map_err(|_| invalid_url())?;
    if text.is_empty()
        || matches!(text.as_str(), "." | "..")
        || text.chars().any(|c| {
            c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | '%' | '?' | '#')
        })
    {
        return Err(invalid_url());
    }
    Ok(text)
}
/// Inspect the raw path before URL parsing can erase dot segments or backslashes.
pub fn parse_url(config: &ForgeConfig, input: &str) -> Result<IssueRoute> {
    if input.len() > 4096
        || input.contains('\\')
        || input.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid_url());
    }
    let (_, tail) = input.split_once("://").ok_or_else(invalid_url)?;
    let (authority, raw_path) = tail.split_once('/').ok_or_else(invalid_url)?;
    if authority.contains('@') {
        return Err(invalid_url());
    }
    let segments: Vec<_> = raw_path
        .split('/')
        .map(decode_segment)
        .collect::<Result<_>>()?;
    let submitted = url::Url::parse(input).map_err(|_| invalid_url())?;
    let base = web_base(config)?;
    if !matches!(submitted.scheme(), "http" | "https")
        || submitted.origin() != base.origin()
        || !submitted.username().is_empty()
        || submitted.password().is_some()
        || submitted.query().is_some()
        || submitted.fragment().is_some()
    {
        return Err(invalid_url());
    }
    let prefix: Vec<_> = base
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(decode_segment)
        .collect::<Result<_>>()?;
    let route = segments
        .strip_prefix(prefix.as_slice())
        .ok_or_else(invalid_url)?;
    let (namespace, repository, number) = if config.kind == Kind::Gitlab {
        if route.len() < 5 || route[route.len() - 3] != "-" || route[route.len() - 2] != "issues" {
            return Err(invalid_url());
        }
        (
            &route[..route.len() - 4],
            &route[route.len() - 4],
            &route[route.len() - 1],
        )
    } else {
        if route.len() != 4 || route[2] != "issues" {
            return Err(invalid_url());
        }
        (&route[..1], &route[1], &route[3])
    };
    if namespace.is_empty()
        || namespace.iter().any(|s| s == "-")
        || number.starts_with('0')
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid_url());
    }
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(invalid_url)?;
    let mut canonical = base;
    canonical.set_query(None);
    canonical.set_fragment(None);
    {
        let mut path = canonical.path_segments_mut().map_err(|_| invalid_url())?;
        path.clear();
        for segment in &segments {
            path.push(segment);
        }
    }
    Ok(IssueRoute {
        owner: namespace.join("/"),
        repository: repository.clone(),
        number,
        url: canonical.to_string(),
    })
}
fn marker(v: &Value) -> bool {
    ["pull_request", "merge_request"]
        .iter()
        .any(|key| v.get(key).is_some_and(|v| !v.is_null() && v != false))
        || v.get("type").and_then(Value::as_str).is_some_and(|s| {
            matches!(
                s,
                "pull" | "pull_request" | "MergeRequest" | "merge_request"
            )
        })
}
fn string<'a>(v: &'a Value, field: &str) -> Result<&'a str> {
    v.get(field)
        .and_then(Value::as_str)
        .ok_or_else(Error::upstream)
}
fn number(v: &Value, field: &str) -> Result<u64> {
    v.get(field)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .ok_or_else(Error::upstream)
}
fn person(v: &Value, kind: Kind) -> Option<Person> {
    let id = v.get("id")?.as_i64()?.to_string();
    if id.starts_with('-') || id == "0" {
        return None;
    }
    let username = v
        .get(if kind == Kind::Gitlab {
            "username"
        } else {
            "login"
        })?
        .as_str()?;
    Some(Person {
        id,
        username: username.into(),
        display_name: v
            .get(if matches!(kind, Kind::Codeberg | Kind::Forgejo) {
                "full_name"
            } else {
                "name"
            })
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(username)
            .into(),
        url: v
            .get(if kind == Kind::Gitlab {
                "web_url"
            } else {
                "html_url"
            })
            .and_then(Value::as_str)
            .and_then(crate::config::safe_url),
    })
}
fn browser_url(v: &Value, kind: Kind) -> Result<&str> {
    string(
        v,
        if kind == Kind::Gitlab {
            "web_url"
        } else {
            "html_url"
        },
    )
}
fn normalize(
    config: &ForgeConfig,
    route: &IssueRoute,
    v: &Value,
    me: &Identity,
) -> Result<UpstreamIssue> {
    if marker(v) {
        return Err(not_issue());
    }
    let returned =
        parse_url(config, browser_url(v, config.kind)?).map_err(|_| Error::upstream())?;
    if &returned != route
        || number(
            v,
            if config.kind == Kind::Gitlab {
                "iid"
            } else {
                "number"
            },
        )? != route.number
    {
        return Err(Error::upstream());
    }
    let state = match string(v, "state")? {
        "open" | "opened" => "open",
        "closed" => "closed",
        _ => return Err(Error::upstream()),
    };
    let assignees = match v.get("assignees") {
        Some(Value::Array(users)) => users
            .iter()
            .map(|u| person(u, config.kind).ok_or_else(Error::upstream))
            .collect::<Result<Vec<_>>>()?,
        Some(Value::Null) if matches!(config.kind, Kind::Codeberg | Kind::Forgejo) => vec![],
        _ => return Err(Error::upstream()),
    };
    let labels = v
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .map(|label| {
                    if config.kind == Kind::Gitlab {
                        label.as_str().ok_or_else(Error::upstream)
                    } else {
                        string(label, "name")
                    }
                    .map(str::to_owned)
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let description = v
        .get(if config.kind == Kind::Gitlab {
            "description"
        } else {
            "body"
        })
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().ok_or_else(Error::upstream))
        .transpose()?
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    Ok(UpstreamIssue {
        id: format!(
            "{}:{}/{}:issue:{}",
            config.id, route.owner, route.repository, route.number
        ),
        source_id: config.id.clone(),
        forge: config.kind,
        number: route.number,
        owner: route.owner.clone(),
        repository: route.repository.clone(),
        title: string(v, "title")?.into(),
        url: route.url.clone(),
        description,
        state: state.into(),
        reporter: v
            .get(if config.kind == Kind::Gitlab {
                "author"
            } else {
                "user"
            })
            .and_then(|v| person(v, config.kind)),
        is_assigned_to_me: assignees.iter().any(|p| p.id == me.account_id),
        assignees,
        labels,
        created_at: serde_json::from_value(
            v.get("created_at").cloned().ok_or_else(Error::upstream)?,
        )
        .map_err(|_| Error::upstream())?,
        updated_at: serde_json::from_value(
            v.get("updated_at").cloned().ok_or_else(Error::upstream)?,
        )
        .map_err(|_| Error::upstream())?,
    })
}
pub async fn fetch(
    config: &ForgeConfig,
    http: &ReadTransport,
    route: &IssueRoute,
    me: &Identity,
) -> Result<UpstreamIssue> {
    check_policy(config, route)?;
    let segment = crate::forge::segment;
    let path = if config.kind == Kind::Gitlab {
        format!(
            "projects/{}/issues/{}",
            segment(&format!("{}/{}", route.owner, route.repository)),
            route.number
        )
    } else {
        format!(
            "repos/{}/{}/issues/{}",
            segment(&route.owner),
            segment(&route.repository),
            route.number
        )
    };
    let v: Value = http.get(&path, &[]).await.map_err(|e| {
        if e.code == "upstream_not_found" {
            not_found()
        } else {
            e
        }
    })?;
    normalize(config, route, &v, me)
}
/// Account issue endpoints avoid GitHub's search ceiling and never scan all issues.
pub async fn discover(
    config: &ForgeConfig,
    http: &ReadTransport,
    me: &Identity,
    progress: &Progress,
) -> Result<Vec<UpstreamIssue>> {
    progress.discovering();
    let (path, params) = match config.kind {
        Kind::Github => (
            "issues",
            vec![("filter", "assigned".into()), ("state", "open".into())],
        ),
        Kind::Gitlab => (
            "issues",
            vec![
                ("scope", "assigned_to_me".into()),
                ("assignee_id", me.account_id.clone()),
                ("state", "opened".into()),
            ],
        ),
        Kind::Codeberg | Kind::Forgejo => (
            "repos/issues/search",
            vec![
                ("assigned", "true".into()),
                ("type", "issues".into()),
                ("state", "open".into()),
            ],
        ),
        Kind::Jira => return Err(Error::upstream()),
    };
    let candidates: Vec<Value> = http
        .complete_pages(path, &params, |v| {
            browser_url(v, config.kind).map(str::to_owned)
        })
        .await?;
    let mut routes = BTreeMap::new();
    for v in candidates {
        if marker(&v) {
            continue;
        }
        let route =
            parse_url(config, browser_url(&v, config.kind)?).map_err(|_| Error::upstream())?;
        if config.includes(&format!("{}/{}", route.owner, route.repository)) {
            routes.insert(
                (route.owner.clone(), route.repository.clone(), route.number),
                route,
            );
        }
    }
    progress.fetching(None);
    let mut issues: Vec<_> = stream::iter(routes.into_values())
        .map(|route| async move {
            let issue = match fetch(config, http, &route, me).await {
                Ok(issue) => Some(issue),
                Err(e) if e.code == "not_upstream_issue" => None,
                Err(e) => return Err(e),
            };
            progress.item_completed();
            Ok::<_, Error>(issue)
        })
        .buffer_unordered(http.limits().max_concurrent_items)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .collect();
    issues.retain(|i| i.state == "open" && i.is_assigned_to_me);
    issues.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(p) = progress.snapshot() {
        progress.fetching(Some(p.completed_items));
    }
    Ok(issues)
}
pub fn warning(issue: &UpstreamIssue) -> Option<String> {
    (issue.state == "closed")
        .then(|| "This upstream issue is closed; you may still create a Jira ticket.".into())
}
pub fn description(issue: &UpstreamIssue) -> String {
    let mut text = issue.description.clone();
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str("Upstream issue: ");
    text.push_str(&issue.url);
    if !issue.labels.is_empty() {
        text.push_str("\nLabels: ");
        text.push_str(&issue.labels.join(", "));
    }
    text
}
struct Entry {
    scope: String,
    issue: UpstreamIssue,
    bytes: usize,
}
#[derive(Default)]
pub struct Resolutions(Mutex<VecDeque<Entry>>);
impl Resolutions {
    pub(crate) fn invalidate_source(&self, source_id: &str) {
        self.0
            .lock()
            .expect("resolution lock poisoned")
            .retain(|e| e.issue.source_id != source_id);
    }
    pub fn insert(&self, scope: String, issue: UpstreamIssue) -> Result<()> {
        let bytes = serde_json::to_vec(&issue)
            .map_err(|_| Error::internal())?
            .len();
        if bytes > 8 * 1024 * 1024 {
            return Err(Error::upstream());
        }
        let mut entries = self.0.lock().expect("resolution lock poisoned");
        entries.retain(|e| e.issue.id != issue.id);
        while entries.len() >= 128
            || entries
                .iter()
                .map(|e| e.bytes)
                .sum::<usize>()
                .saturating_add(bytes)
                > 8 * 1024 * 1024
        {
            entries.pop_front();
        }
        entries.push_back(Entry {
            scope,
            issue,
            bytes,
        });
        Ok(())
    }
    pub fn select(&self, id: &str, snapshot: &Snapshot) -> Result<UpstreamIssue> {
        let mut entries = self.0.lock().expect("resolution lock poisoned");
        entries.retain(|e| snapshot.source_accounts.get(&e.issue.source_id) == Some(&e.scope));
        entries
            .iter()
            .rev()
            .find(|e| e.issue.id == id)
            .map(|e| &e.issue)
            .or_else(|| snapshot.board.upstream_issues.iter().find(|i| i.id == id))
            .cloned()
            .ok_or_else(not_found)
    }
}
