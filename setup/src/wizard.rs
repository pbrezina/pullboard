//! Linear setup: prerequisites → Jira → forges → AI → runtime → authorization.
use crate::{
    cli::{Cli, JiraArgs, jira_cloud_url, semantic_field},
    discovery::{self, BoardTeams, Catalog, GetJson, JiraClient, Metadata},
    install::{self, Channel, Installer, Paths},
    prompt::{Back, Cancelled, Choice, OpenMenu, Prompts, TerminalPrompts, clean},
};
use anyhow::{Context, Result, ensure};
use pullboard_core::{
    config::{
        AiConfig, AiProvider, Config, ForgeConfig, JiraConfig, PurposeDefaults, Secret,
        SprintPattern, Upstream,
    },
    model::Kind,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io::IsTerminal,
    sync::{Arc, OnceLock},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

mod jira_args;

/// Scriptable, read-only discovery. Recents never imply a complete inventory.
pub trait Discovery {
    fn catalog(&self, jira: &JiraConfig) -> impl Future<Output = Result<Catalog>>;
    fn identity(&self, jira: &JiraConfig) -> impl Future<Output = Result<String>>;
    fn boards(&self, jira: &JiraConfig) -> impl Future<Output = Result<Vec<Choice>>>;
    fn all_projects(&self, jira: &JiraConfig) -> impl Future<Output = Result<Vec<Choice>>>;
    fn project(&self, jira: &JiraConfig, input: &str) -> impl Future<Output = Result<Vec<Choice>>>;
    fn board(&self, jira: &JiraConfig, input: &str) -> impl Future<Output = Result<Choice>>;
    fn project_boards(&self, jira: &JiraConfig) -> impl Future<Output = Result<Vec<Choice>>>;
    fn settings(&self, jira: &JiraConfig) -> impl Future<Output = Result<Catalog>>;
    fn own_groups(&self, jira: &JiraConfig) -> impl Future<Output = Result<Vec<Choice>>>;
    fn all_groups(&self, jira: &JiraConfig) -> impl Future<Output = Result<Vec<Choice>>>;
    fn group(&self, jira: &JiraConfig, name: &str) -> impl Future<Output = Result<Choice>>;
    fn metadata(&self, jira: &JiraConfig) -> impl Future<Output = Result<Metadata>>;
    fn board_teams(
        &self,
        jira: &JiraConfig,
        metadata: &Metadata,
    ) -> impl Future<Output = Result<BoardTeams>>;
    fn forge(&self, forge: &ForgeConfig) -> impl Future<Output = Result<String>>;
    fn ai_test(&self, ai: &AiConfig) -> impl Future<Output = Result<()>>;
}
// Deliberately not Debug: the comparison key contains credentials. Only one
// credential scope (and its bounded in-memory client cache) survives at a time.
struct JiraSession {
    scope: Zeroizing<String>,
    client: Arc<JiraClient>,
}
struct LiveDiscovery {
    limits: Upstream,
    jira: Mutex<Option<JiraSession>>,
}
impl LiveDiscovery {
    fn new(limits: Upstream) -> Self {
        Self {
            limits,
            jira: Mutex::new(None),
        }
    }
    async fn client(&self, j: &JiraConfig) -> Result<Arc<JiraClient>> {
        let scope = Zeroizing::new(serde_json::to_string(&(&j.base_url, &j.email, &j.token))?);
        let mut session = self.jira.lock().await;
        if let Some(saved) = session.as_ref().filter(|saved| saved.scope == scope) {
            return Ok(Arc::clone(&saved.client));
        }
        // Invalidate before construction too: a failed new scope must not expose
        // any previous account's cached entries.
        *session = None;
        let client = Arc::new(JiraClient::new(j, &self.limits).await?);
        *session = Some(JiraSession {
            scope,
            client: Arc::clone(&client),
        });
        Ok(client)
    }
}
impl Discovery for LiveDiscovery {
    async fn catalog(&self, j: &JiraConfig) -> Result<Catalog> {
        discovery::catalog_from(self.client(j).await?.as_ref()).await
    }
    async fn identity(&self, j: &JiraConfig) -> Result<String> {
        // catalog_from also reads recents. If that optional endpoint fails,
        // verify identity independently so manual setup remains available.
        let me = self
            .client(j)
            .await?
            .get_json("rest/api/3/myself", &[])
            .await
            .context("verifying Jira identity")?;
        discovery::text(&me, "accountId")?;
        ensure!(
            me.get("active").and_then(serde_json::Value::as_bool) == Some(true)
                && me.get("accountType").and_then(serde_json::Value::as_str) != Some("app"),
            "Jira identity must be an active human account"
        );
        Ok(discovery::text(&me, "displayName")?.into())
    }
    async fn boards(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        discovery::boards_from(self.client(j).await?.as_ref()).await
    }
    async fn all_projects(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        discovery::all_projects_from(self.client(j).await?.as_ref()).await
    }
    async fn project(&self, j: &JiraConfig, input: &str) -> Result<Vec<Choice>> {
        discovery::project_from(self.client(j).await?.as_ref(), j, input).await
    }
    async fn board(&self, j: &JiraConfig, input: &str) -> Result<Choice> {
        discovery::board_from(self.client(j).await?.as_ref(), j, input).await
    }
    async fn project_boards(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        discovery::project_boards_from(self.client(j).await?.as_ref(), j).await
    }
    async fn settings(&self, j: &JiraConfig) -> Result<Catalog> {
        discovery::settings_from(self.client(j).await?.as_ref()).await
    }
    async fn own_groups(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        discovery::own_groups_from(self.client(j).await?.as_ref()).await
    }
    async fn all_groups(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        discovery::all_groups_from(self.client(j).await?.as_ref()).await
    }
    async fn group(&self, j: &JiraConfig, name: &str) -> Result<Choice> {
        discovery::group_from(self.client(j).await?.as_ref(), name).await
    }
    async fn metadata(&self, j: &JiraConfig) -> Result<Metadata> {
        discovery::metadata_from(self.client(j).await?.as_ref(), j).await
    }
    async fn board_teams(&self, j: &JiraConfig, metadata: &Metadata) -> Result<BoardTeams> {
        discovery::board_teams_from(self.client(j).await?.as_ref(), j, metadata).await
    }
    async fn forge(&self, f: &ForgeConfig) -> Result<String> {
        discovery::forge_identity(f, &self.limits).await
    }
    async fn ai_test(&self, a: &AiConfig) -> Result<()> {
        discovery::synthetic_ai_test(a, &self.limits).await
    }
}

async fn cancellable<T>(
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(Cancelled.into()), result = work => result }
}

enum Connected<T> {
    Ready(T),
    Change,
    Skip,
}
async fn connect<T, F: Future<Output = Result<T>>>(
    p: &mut impl Prompts,
    cancel: &CancellationToken,
    jira: Option<&JiraConfig>,
    mut work: impl FnMut() -> F,
) -> Result<Connected<T>> {
    loop {
        match cancellable(cancel, work()).await {
            Ok(value) => return Ok(Connected::Ready(value)),
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => {
                if let Some(jira) = jira {
                    jira_error(p, "Connection/discovery failed", &error, jira);
                } else {
                    p.say("Connection/discovery failed or was incomplete. Check permissions and answers; nothing unverified will be saved.");
                }
            }
        }
        match p
            .select(
                "Connection failed:",
                &choices(&["Retry", "Change answers", "Skip integration"]),
                0,
            )
            .await?
        {
            0 => {}
            1 => return Ok(Connected::Change),
            _ => return Ok(Connected::Skip),
        }
    }
}

fn choices(labels: &[&str]) -> Vec<Choice> {
    labels.iter().map(|s| Choice::new(*s, *s)).collect()
}

fn jira_error(p: &mut impl Prompts, operation: &str, error: &anyhow::Error, jira: &JiraConfig) {
    p.say(&format!(
        "{operation}: {}. Nothing unverified will be saved.",
        safe_jira_error(error, jira)
    ));
}

fn basic_material(bytes: &[u8]) -> Zeroizing<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Zeroizing::new(String::new());
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(char::from(ALPHABET[usize::from(a >> 2)]));
        out.push(char::from(ALPHABET[usize::from((a & 3) << 4 | b >> 4)]));
        out.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from((b & 15) << 2 | c >> 6)])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(c & 63)])
        } else {
            '='
        });
    }
    out
}

fn decode_error_percent(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.as_bytes();
    while let Some((&first, rest)) = bytes.split_first() {
        if first == b'%'
            && let [a, b, tail @ ..] = rest
            && let (Some(a), Some(b)) = (char::from(*a).to_digit(16), char::from(*b).to_digit(16))
            && let Ok(byte) = u8::try_from(a * 16 + b)
        {
            out.push(byte);
            bytes = tail;
            continue;
        }
        out.push(first);
        bytes = rest;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Transport diagnostics are deliberately safe; do not format arbitrary error
/// chains. Raw reqwest causes are classified without their URL/body Display.
/// Redaction here also protects user-provided/manual-input validation contexts.
fn safe_jira_error(error: &anyhow::Error, jira: &JiraConfig) -> String {
    let mut redactions = Vec::new();
    if let Secret::Literal { value } = &jira.token
        && !value.is_empty()
    {
        redactions.push(Zeroizing::new(value.clone()));
        let basic = Zeroizing::new(format!("{}:{value}", jira.email));
        for secret in [value.as_str(), basic.as_str()] {
            let encoded = basic_material(secret.as_bytes());
            for encoded in [
                encoded.to_string(),
                encoded.replace('+', "-").replace('/', "_"),
            ] {
                redactions.push(Zeroizing::new(encoded.trim_end_matches('=').to_owned()));
                redactions.push(Zeroizing::new(encoded));
            }
        }
        redactions.push(basic);
        let encoded = Zeroizing::new(
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>(),
        );
        redactions.push(Zeroizing::new(encoded.to_lowercase()));
        redactions.push(encoded);
        // Also redact percent-encoding of otherwise unreserved token bytes.
        redactions.push(Zeroizing::new(
            value.bytes().map(|b| format!("%{b:02X}")).collect(),
        ));
        redactions.push(Zeroizing::new(
            value.bytes().map(|b| format!("%{b:02x}")).collect(),
        ));
    }
    redactions.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let messages = if let Some(raw) = error.downcast_ref::<reqwest::Error>() {
        vec![if raw.is_timeout() {
            "Jira request timed out".into()
        } else if let Some(status) = raw.status() {
            format!("Jira returned HTTP {}", status.as_u16())
        } else {
            "Jira request failed; check connectivity and permissions".into()
        }]
    } else {
        let mut messages = vec![error.to_string()];
        // The transport exposes a deliberately safe type. Discovery's own root
        // errors explain validation/pagination; sanitize those too. Never dump a
        // Debug representation or an arbitrary formatted reqwest source chain.
        let cause = error
            .downcast_ref::<pullboard_core::transport::ReadDiagnosticError>()
            .map(ToString::to_string)
            .unwrap_or_else(|| error.root_cause().to_string());
        if messages[0] != cause {
            messages.push(cause);
        }
        messages
    };
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    let url = URL.get_or_init(|| regex::Regex::new(r"(?i)https?://[^\s]+").expect("static regex"));
    let mut parts = Vec::new();
    for mut message in messages {
        // Normalize before matching, so stripping a control character cannot
        // reconstruct a credential that was split in the untrusted diagnostic.
        for _ in 0..2 {
            message = decode_error_percent(&message);
        }
        message = clean(&message);
        for secret in &redactions {
            message = message.replace(secret.as_str(), "[REDACTED]");
        }
        // Never show HTML/JSON response bodies, auth headers, or embedded URLs.
        // Known discovery/transport errors carry structured prose, not bodies.
        if message.contains(['<', '>', '{', '}'])
            || message.to_ascii_lowercase().contains("authorization:")
            || message.to_ascii_lowercase().contains("cookie:")
        {
            message = if let Some(status) = error
                .downcast_ref::<pullboard_core::transport::ReadDiagnosticError>()
                .and_then(|error| error.status)
            {
                format!(
                    "Jira returned HTTP {}; unsafe upstream response details withheld",
                    status.as_u16()
                )
            } else {
                "unsafe upstream response details withheld".into()
            };
        }
        message = url.replace_all(&message, "[URL withheld]").into_owned();
        message = message
            .split_whitespace()
            .map(|word| {
                if word.contains('%') || word.to_ascii_lowercase().starts_with("www.") {
                    "[details withheld]"
                } else {
                    word
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(512)
            .collect();
        if !message.is_empty() && parts.last() != Some(&message) {
            parts.push(message);
        }
    }
    if parts.is_empty() {
        return "Jira discovery failed; check credentials, permissions and pagination bounds"
            .into();
    }
    parts.join(": ").chars().take(1600).collect()
}

fn merge_choices(items: &mut Vec<Choice>, more: Vec<Choice>) {
    for choice in more {
        if let Some(saved) = items.iter_mut().find(|saved| saved.id == choice.id) {
            *saved = choice;
        } else {
            items.push(choice);
        }
    }
}

async fn verify_saved_projects(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &JiraConfig,
    items: &mut Vec<Choice>,
) -> Result<()> {
    for key in &j.projects {
        if items.iter().any(|item| &item.id == key) {
            continue;
        }
        let result = cancellable(cancel, async {
            let found = d.project(j, key).await?;
            found
                .into_iter()
                .find(|item| &item.id == key)
                .context("saved project lookup did not verify its exact key")
        })
        .await;
        match result {
            Ok(item) => merge_choices(items, vec![item]),
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => jira_error(
                p,
                &format!(
                    "Saved project {} could not be verified; retry or explicitly remove it",
                    clean(key)
                ),
                &error,
                j,
            ),
        }
    }
    Ok(())
}

async fn select_projects(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    original: &Config,
    mut items: Vec<Choice>,
) -> Result<Connected<()>> {
    p.say("Recent Jira projects are non-exhaustive. Add a project manually or explicitly load all accessible projects; setup never scans automatically.");
    for item in &items {
        p.say(&format!("Recent project: {}", clean(&item.label)));
    }
    verify_saved_projects(p, d, cancel, j, &mut items).await?;
    let previous_primary = j.projects.first().cloned();
    let mut next_action = (!items.is_empty() || !j.projects.is_empty()).then_some(0);
    loop {
        p.say(&format!(
            "Selected projects (primary first): {}",
            if j.projects.is_empty() {
                "none".into()
            } else {
                j.projects.join(", ")
            }
        ));
        if items.is_empty() {
            p.say("No recent/verified projects are available. Manual project entry is still available.");
        }
        let actions = choices(&[
            "Choose projects from the recent/verified list",
            "Add project by key, exact name or project URL",
            "Load all accessible projects (slow; confirmation required)",
            "Retry recent projects and saved selection verification",
            "Continue with selected projects",
            "Change Jira connection answers",
            "Skip Jira",
        ]);
        let action = if let Some(action) = next_action.take() {
            action
        } else {
            p.select(
                "Project actions:",
                &actions,
                if !j.projects.is_empty() {
                    4
                } else if items.is_empty() {
                    1
                } else {
                    0
                },
            )
            .await?
        };
        match action {
            0 => {
                if items.is_empty() && j.projects.is_empty() {
                    continue;
                }
                // Failed saved selections are shown as unverified, never silently
                // discarded just because Jira omitted them from recent results.
                let mut display = items.clone();
                for key in &j.projects {
                    if !display.iter().any(|item| &item.id == key) {
                        display.push(Choice::new(
                            key,
                            format!("{key} (saved; NOT verified — retry or remove)"),
                        ));
                    }
                }
                let selected =
                    match inventory_many(p, "Jira projects:", &display, &j.projects).await {
                        Ok(selected) => selected,
                        Err(error) if error.is::<OpenMenu>() => continue,
                        Err(error) => return Err(error),
                    };
                if selected.len() > 100 {
                    p.say("Select at most 100 Jira projects.");
                    next_action = Some(0);
                    continue;
                }
                let changes = project_cleanup(original, &selected);
                if !changes.is_empty() {
                    for change in &changes {
                        p.say(change);
                    }
                    if !p
                        .confirm(
                            "Confirm project removal and this dependent-setting cleanup?",
                            false,
                        )
                        .await?
                    {
                        next_action = Some(0);
                        continue;
                    }
                }
                let mut ordered: Vec<_> = j
                    .projects
                    .iter()
                    .filter(|key| selected.contains(key))
                    .cloned()
                    .collect();
                ordered.extend(selected.into_iter().filter(|key| !j.projects.contains(key)));
                j.projects = ordered;
                next_action = Some(4);
            }
            1 => {
                let input = p
                    .text(
                        "Project key, exact name or same-tenant project URL (blank returns):",
                        "",
                    )
                    .await?;
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }
                let result = if let Some(known) = items.iter().find(|item| item.id == input) {
                    Ok(vec![known.clone()])
                } else {
                    cancellable(cancel, d.project(j, input)).await
                };
                match result {
                    Ok(found) if !found.is_empty() => {
                        let key = if let [only] = found.as_slice() {
                            p.say(&format!("Verified project: {}", clean(&only.label)));
                            only.id.clone()
                        } else {
                            one(
                                p,
                                "Several projects have this exact name; choose by key:",
                                &found,
                                "",
                            )
                            .await?
                        };
                        if !j.projects.contains(&key) {
                            if j.projects.len() >= 100 {
                                p.say("Select at most 100 Jira projects.");
                                continue;
                            }
                            j.projects.push(key);
                        }
                        merge_choices(&mut items, found);
                    }
                    Ok(_) => {
                        p.say("No exact project match was verified. Try its project key or URL.")
                    }
                    Err(error) if error.is::<Cancelled>() => return Err(error),
                    Err(error) => jira_error(p, "Project lookup failed", &error, j),
                }
            }
            2 => {
                p.say("Warning: loading all accessible projects may require many Jira requests and take a long time on a large tenant. Jira API limits may be reached. This is read-only and never runs automatically.");
                if p.confirm("Load ALL accessible Jira projects now?", false)
                    .await?
                {
                    match cancellable(cancel, d.all_projects(j)).await {
                        Ok(found) => {
                            p.say(&format!("Loaded {} accessible projects; existing selections are unchanged. Choose from the list to edit them.", found.len()));
                            merge_choices(&mut items, found);
                        }
                        Err(error) if error.is::<Cancelled>() => return Err(error),
                        Err(error) => {
                            jira_error(p, "All-project lookup failed or was incomplete", &error, j)
                        }
                    }
                }
            }
            3 => {
                match cancellable(cancel, d.catalog(j)).await {
                    Ok(recent) => merge_choices(&mut items, recent.projects),
                    Err(error) if error.is::<Cancelled>() => return Err(error),
                    Err(error) => jira_error(
                        p,
                        "Recent project lookup failed; manual entry remains available",
                        &error,
                        j,
                    ),
                }
                verify_saved_projects(p, d, cancel, j, &mut items).await?;
            }
            4 => {
                if j.projects.is_empty() {
                    p.say("Select or add at least one verified project before continuing.");
                    continue;
                }
                if j.projects
                    .iter()
                    .any(|key| !items.iter().any(|item| &item.id == key))
                {
                    p.say("Some saved projects are not verified. Retry, add them manually, or explicitly remove them before continuing.");
                    continue;
                }
                let selected: Vec<_> = items
                    .iter()
                    .filter(|item| j.projects.contains(&item.id))
                    .cloned()
                    .collect();
                let primary = one(
                    p,
                    "Primary Jira project:",
                    &selected,
                    previous_primary.as_deref().unwrap_or(""),
                )
                .await?;
                j.projects.retain(|key| key != &primary);
                j.projects.insert(0, primary);
                j.project_overrides
                    .retain(|key, _| j.projects.contains(key));
                return Ok(Connected::Ready(()));
            }
            5 => return Ok(Connected::Change),
            6 => return Ok(Connected::Skip),
            _ => anyhow::bail!("invalid project action"),
        }
    }
}

async fn recent_boards(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &JiraConfig,
    items: &mut Vec<Choice>,
    verified: &mut BTreeSet<String>,
) -> Result<()> {
    match cancellable(cancel, d.boards(j)).await {
        Ok(recent) => {
            for item in &recent {
                p.say(&format!("Recent board: {}", clean(&item.label)));
            }
            merge_choices(items, recent);
        }
        Err(error) if error.is::<Cancelled>() => return Err(error),
        Err(error) => jira_error(
            p,
            "Recent board lookup failed; manual entry remains available",
            &error,
            j,
        ),
    }
    for id in &j.board_ids {
        if verified.contains(&id.to_string()) {
            continue;
        }
        match cancellable(cancel, d.board(j, &id.to_string())).await {
            Ok(board) if board.id == id.to_string() => {
                verified.insert(board.id.clone());
                merge_choices(items, vec![board]);
            }
            Ok(_) => {
                p.say("Saved board lookup returned a different ID; retry or explicitly remove it.")
            }
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => jira_error(
                p,
                "Saved board could not be verified; retry or explicitly remove it",
                &error,
                j,
            ),
        }
    }
    Ok(())
}

async fn select_board(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
) -> Result<Connected<()>> {
    p.say("Recent Jira boards are non-exhaustive. Manual entry is available; broader lookup is restricted to your selected projects, never all tenant boards.");
    let mut items = Vec::new();
    let mut verified_ids = BTreeSet::new();
    recent_boards(p, d, cancel, j, &mut items, &mut verified_ids).await?;
    let mut next_action = (!items.is_empty()).then_some(0);
    loop {
        let selected = !j.board_ids.is_empty();
        p.say(&format!(
            "Selected boards (My board first): {:?}",
            j.board_ids
        ));
        if items.is_empty() {
            p.say("No recent/verified boards are available. Add a board manually or explicitly load boards for selected projects.");
        }
        let action = if let Some(action) = next_action.take() {
            action
        } else {
            p.select(
                "Board actions:",
                &choices(&[
                    "Choose boards from the recent/verified list (add/remove)",
                    "Add board by ID or board URL",
                    "Load all boards for selected projects (confirmation required)",
                    "Retry recent boards and saved selection verification",
                    "Continue with selected boards and choose My board",
                    "Change Jira connection answers",
                    "Skip Jira",
                ]),
                if selected {
                    4
                } else if items.is_empty() {
                    1
                } else {
                    0
                },
            )
            .await?
        };
        match action {
            0 => {
                if items.is_empty() && j.board_ids.is_empty() {
                    continue;
                }
                let saved: Vec<_> = j.board_ids.iter().map(u64::to_string).collect();
                let mut display = items.clone();
                for id in &saved {
                    if !display.iter().any(|item| &item.id == id) {
                        display.push(Choice::new(
                            id,
                            format!("{id} (saved; NOT verified — retry or remove)"),
                        ));
                    }
                }
                match inventory_many(p, "Jira boards:", &display, &saved).await {
                    Ok(selected) => {
                        if selected.len() > 100 {
                            p.say("Select at most 100 Jira boards.");
                            continue;
                        }
                        let mut ordered: Vec<_> = saved
                            .iter()
                            .filter(|id| selected.contains(id))
                            .cloned()
                            .collect();
                        ordered.extend(selected.into_iter().filter(|id| !saved.contains(id)));
                        j.board_ids = ordered
                            .iter()
                            .map(|id| id.parse().context("invalid discovered board ID"))
                            .collect::<Result<_>>()?;
                        next_action = Some(4);
                    }
                    Err(error) if error.is::<OpenMenu>() => {}
                    Err(error) => return Err(error),
                }
            }
            1 => {
                let input = p
                    .text("Board ID or same-tenant board URL (blank returns):", "")
                    .await?;
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }
                let result = if verified_ids.contains(input) {
                    items
                        .iter()
                        .find(|item| item.id == input)
                        .cloned()
                        .context("missing verified board")
                } else {
                    cancellable(cancel, d.board(j, input)).await
                };
                match result {
                    Ok(board) => {
                        verified_ids.insert(board.id.clone());
                        let id = board.id.parse().context("invalid verified board ID")?;
                        if !j.board_ids.contains(&id) {
                            if j.board_ids.len() >= 100 {
                                p.say("Select at most 100 Jira boards.");
                                continue;
                            }
                            j.board_ids.push(id);
                        }
                        merge_choices(&mut items, vec![board]);
                    }
                    Err(error) if error.is::<Cancelled>() => return Err(error),
                    Err(error) => jira_error(p, "Board lookup failed", &error, j),
                }
            }
            2 => {
                p.say(&format!("Warning: loading every board for selected projects [{}] may take a long time and reach Jira API limits. Shared boards are deduplicated; global boards are never enumerated.", j.projects.join(", ")));
                if p.confirm("Load ALL boards for selected projects now?", false)
                    .await?
                {
                    match cancellable(cancel, d.project_boards(j)).await {
                        Ok(found) => {
                            p.say(&format!(
                                "Loaded {} selected-project boards; choose one from the list.",
                                found.len()
                            ));
                            merge_choices(&mut items, found);
                        }
                        Err(error) if error.is::<Cancelled>() => return Err(error),
                        Err(error) => jira_error(
                            p,
                            "Selected-project board lookup failed or was incomplete",
                            &error,
                            j,
                        ),
                    }
                }
            }
            3 => recent_boards(p, d, cancel, j, &mut items, &mut verified_ids).await?,
            4 if selected => {
                // A recent listing or saved ID is not proof: verify every selected
                // board with this connection, including boards omitted by recents.
                let mut verified = Vec::new();
                for id in &j.board_ids {
                    let id = id.to_string();
                    if verified_ids.contains(&id) {
                        verified.push(
                            items
                                .iter()
                                .find(|item| item.id == id)
                                .context("missing verified board")?
                                .clone(),
                        );
                        continue;
                    }
                    match cancellable(cancel, d.board(j, &id)).await {
                        Ok(board) if board.id == id => {
                            verified_ids.insert(id);
                            merge_choices(&mut items, vec![board.clone()]);
                            verified.push(board);
                        }
                        Ok(_) => p.say("Board lookup returned a different ID; retry or remove it."),
                        Err(error) if error.is::<Cancelled>() => return Err(error),
                        Err(error) => jira_error(
                            p,
                            "Selected board could not be verified; retry or remove it",
                            &error,
                            j,
                        ),
                    }
                }
                if verified.len() != j.board_ids.len() {
                    continue;
                }
                let primary = one(p, "My board:", &verified, &j.board_ids[0].to_string()).await?;
                let primary = primary.parse().context("invalid verified board ID")?;
                j.board_ids.retain(|id| *id != primary);
                j.board_ids.insert(0, primary);
                return Ok(Connected::Ready(()));
            }
            4 => p.say("Select or add at least one verified board before continuing."),
            5 => return Ok(Connected::Change),
            6 => return Ok(Connected::Skip),
            _ => anyhow::bail!("invalid board action"),
        }
    }
}

async fn inventory_many(
    p: &mut impl Prompts,
    message: &str,
    items: &[Choice],
    saved: &[String],
) -> Result<Vec<String>> {
    let mut defaults: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, c)| saved.contains(&c.id))
        .map(|(i, _)| i)
        .collect();
    // Keep the convenient observed singleton default, but still show its picker
    // so Ctrl-O is available even when Jira returns just one recent item.
    if items.len() == 1 && saved.is_empty() {
        defaults.push(0);
    }
    loop {
        let selected = p.pick_many(message, items, &defaults).await?;
        if selected.is_empty() {
            p.say("Select at least one item, or use Ctrl-O for other discovery methods.");
            continue;
        }
        return selected
            .into_iter()
            .map(|i| Ok(items.get(i).context("invalid selection")?.id.clone()))
            .collect();
    }
}

async fn load_groups(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &JiraConfig,
    items: &mut Vec<Choice>,
) -> Result<()> {
    match cancellable(cancel, d.own_groups(j)).await {
        Ok(found) => merge_choices(items, found),
        Err(error) if error.is::<Cancelled>() => return Err(error),
        Err(error) => jira_error(
            p,
            "Your group discovery failed; manual lookup remains available",
            &error,
            j,
        ),
    }
    for name in &j.assignee_groups {
        if items.iter().any(|item| &item.id == name) {
            continue;
        }
        match cancellable(cancel, d.group(j, name)).await {
            Ok(item) if &item.id == name => merge_choices(items, vec![item]),
            Ok(_) => p.say(
                "Saved group lookup did not verify its exact name; retry or explicitly remove it.",
            ),
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => jira_error(
                p,
                &format!(
                    "Saved group {} could not be verified; retry or explicitly remove it",
                    clean(name)
                ),
                &error,
                j,
            ),
        }
    }
    Ok(())
}

async fn select_groups(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
) -> Result<Connected<()>> {
    p.say("Discovering your Jira groups. Ctrl-O opens manual lookup, retry and an explicitly confirmed all-groups scan; setup never scans all groups automatically.");
    let mut items = Vec::new();
    load_groups(p, d, cancel, j, &mut items).await?;
    let mut next_action = (!items.is_empty() || !j.assignee_groups.is_empty()).then_some(0);
    loop {
        let action = if let Some(action) = next_action.take() {
            action
        } else {
            if items.is_empty() {
                p.say("No verified groups are available; manual lookup remains available.");
            }
            p.select(
                "Group actions:",
                &choices(&[
                    "Choose from your groups and verified selections",
                    "Add group by exact name",
                    "Load all accessible groups (slow; confirmation required)",
                    "Retry your groups and saved selection verification",
                    "Continue with selected groups",
                    "Change Jira connection answers",
                    "Skip Jira",
                ]),
                if j.assignee_groups.is_empty() { 0 } else { 4 },
            )
            .await?
        };
        match action {
            0 => {
                let mut display = items.clone();
                for name in &j.assignee_groups {
                    if !display.iter().any(|item| &item.id == name) {
                        display.push(Choice::new(
                            name,
                            format!("{name} (saved; NOT verified — retry or remove)"),
                        ));
                    }
                }
                if display.is_empty() {
                    continue;
                }
                let selected = match inventory_many(
                    p,
                    "Assignee groups (at least one):",
                    &display,
                    &j.assignee_groups,
                )
                .await
                {
                    Ok(selected) => selected,
                    Err(error) if error.is::<OpenMenu>() => continue,
                    Err(error) => return Err(error),
                };
                let removed = j
                    .assignee_groups
                    .iter()
                    .any(|name| !selected.contains(name));
                if removed
                    && !p
                        .confirm("Confirm removal of saved assignee groups?", false)
                        .await?
                {
                    next_action = Some(0);
                    continue;
                }
                j.assignee_groups = selected;
                next_action = Some(4);
            }
            1 => {
                let name = p.text("Exact group name (blank returns):", "").await?;
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                let found = cancellable(cancel, d.group(j, name)).await;
                match found {
                    Ok(item) if item.id == name => {
                        if !j.assignee_groups.contains(&item.id) {
                            j.assignee_groups.push(item.id.clone());
                        }
                        merge_choices(&mut items, vec![item]);
                    }
                    Ok(_) => p.say("Group lookup did not verify the exact name."),
                    Err(error) if error.is::<Cancelled>() => return Err(error),
                    Err(error) => jira_error(p, "Group lookup failed", &error, j),
                }
            }
            2 => {
                p.say("Warning: loading all accessible groups may take a long time and reach Jira API limits. This read-only scan never runs automatically.");
                if p.confirm("Load ALL accessible Jira groups now?", false)
                    .await?
                {
                    match cancellable(cancel, d.all_groups(j)).await {
                        Ok(found) => merge_choices(&mut items, found),
                        Err(error) if error.is::<Cancelled>() => return Err(error),
                        Err(error) => {
                            jira_error(p, "All-group lookup failed or was incomplete", &error, j)
                        }
                    }
                }
            }
            3 => load_groups(p, d, cancel, j, &mut items).await?,
            4 => {
                if j.assignee_groups.is_empty() {
                    p.say("Select or add at least one verified group before continuing.");
                } else if j
                    .assignee_groups
                    .iter()
                    .any(|name| !items.iter().any(|item| &item.id == name))
                {
                    p.say("Some saved groups are not verified. Retry, add them manually, or explicitly remove them before continuing.");
                } else {
                    return Ok(Connected::Ready(()));
                }
            }
            5 => return Ok(Connected::Change),
            6 => return Ok(Connected::Skip),
            _ => anyhow::bail!("invalid group action"),
        }
    }
}

fn metadata_teams(j: &JiraConfig, metadata: &Metadata) -> Vec<Choice> {
    if metadata.board_ids != j.board_ids || metadata.team_field_name != j.team_field_name {
        return vec![];
    }
    let names: BTreeSet<_> = metadata
        .types
        .iter()
        .filter(|t| j.projects.contains(&t.project))
        .flat_map(|t| t.teams.iter().cloned())
        .collect();
    names
        .into_iter()
        .map(|name| {
            let scopes = metadata
                .types
                .iter()
                .filter(|t| j.projects.contains(&t.project) && t.teams.contains(&name))
                .map(|t| format!("{}/{}", t.project, t.name))
                .collect::<Vec<_>>()
                .join(", ");
            Choice::new(&name, format!("{name} [{scopes}]"))
        })
        .collect()
}

fn sprint_preview(p: &mut impl Prompts, j: &JiraConfig, metadata: &Metadata) -> usize {
    let current = metadata
        .sprints
        .iter()
        .filter(|s| j.current_sprint(s.board_id, &s.state, &s.name))
        .count();
    for (included, label) in [(true, "Included creation"), (false, "Excluded creation")] {
        for (state, heading) in [("active", "Active"), ("future", "Future")] {
            p.say(&format!("{label} sprints — {heading}:"));
            let mut found = false;
            for s in metadata
                .sprints
                .iter()
                .filter(|s| s.state == state && j.allows_sprint(&s.name) == included)
            {
                found = true;
                p.say(&format!(
                    "{label} sprint: {} — {} (board {}){}",
                    s.state,
                    s.name,
                    s.board_id,
                    if j.current_sprint(s.board_id, &s.state, &s.name) {
                        " — My current sprint match"
                    } else {
                        ""
                    }
                ));
            }
            if !found {
                p.say("None.");
            }
        }
    }
    p.say(&format!("{current} matching active sprint(s) on My board using My pattern. New tickets still start with No sprint."));
    current
}

async fn sprint_pattern_input(
    p: &mut impl Prompts,
    message: &str,
    default: &str,
) -> Result<SprintPattern> {
    loop {
        let value = p.text(message, default).await?;
        match SprintPattern::new(&value) {
            Ok(pattern) => return Ok(pattern),
            Err(_) => p.say("Enter a bounded Rust regex search; case-sensitive unless (?i). Anchors are optional; no percent tokens."),
        }
    }
}

async fn select_sprint_patterns(
    p: &mut impl Prompts,
    j: &mut JiraConfig,
    metadata: &Metadata,
    args: &mut JiraArgs,
) -> Result<()> {
    ensure!(
        metadata.board_ids == j.board_ids,
        "sprint metadata does not match selected boards"
    );
    p.say("Sprint patterns are bounded Rust regex searches: case-sensitive unless (?i), optional anchors, no percent-token substitutions. My board + My pattern determine current sprints; ANY pattern allows creation choices across ALL selected boards.");
    let mut supplied = !args.sprint_name_patterns.is_empty();
    if supplied {
        j.sprint_name_patterns
            .clone_from(&args.sprint_name_patterns);
    }
    let mut edit_primary = !supplied;
    loop {
        if edit_primary {
            let default = if let Some(saved) = j.sprint_name_patterns.first() {
                saved.as_str().to_owned()
            } else {
                let team = regex::escape(&j.team);
                let pattern = SprintPattern::new(&team)?;
                let active: Vec<_> = metadata
                    .sprints
                    .iter()
                    .filter(|s| s.state == "active" && Some(&s.board_id) == j.board_ids.first())
                    .collect();
                if active.iter().any(|s| pattern.is_match(&s.name)) {
                    team
                } else if let [only] = active.as_slice() {
                    regex::escape(&only.name)
                } else {
                    String::new()
                }
            };
            let pattern =
                sprint_pattern_input(p, "Regular expression to identify your sprints:", &default)
                    .await?;
            if j.sprint_name_patterns
                .iter()
                .skip(1)
                .any(|p| p.as_str() == pattern.as_str())
            {
                p.say("Sprint patterns must be unique; choose My pattern from the list instead.");
                continue;
            }
            if let Some(primary) = j.sprint_name_patterns.first_mut() {
                *primary = pattern;
            } else {
                j.sprint_name_patterns.push(pattern);
            }
            edit_primary = false;
        }
        let current = sprint_preview(p, j, metadata);
        let action = if supplied {
            4
        } else {
            p.say(&format!(
                "Sprint patterns (My pattern first): {}",
                j.sprint_name_patterns
                    .iter()
                    .map(SprintPattern::as_str)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            p.select(
                "Sprint pattern actions:",
                &choices(&[
                    "Add sprint pattern",
                    "Edit sprint pattern",
                    "Remove sprint pattern",
                    "Choose My sprint pattern",
                    "Continue with these patterns",
                ]),
                4,
            )
            .await?
        };
        match action {
            0 => {
                if j.sprint_name_patterns.len() >= 100 {
                    p.say("Select at most 100 sprint patterns.");
                    continue;
                }
                let pattern = sprint_pattern_input(p, "Additional sprint regex:", "").await?;
                if j.sprint_name_patterns
                    .iter()
                    .any(|p| p.as_str() == pattern.as_str())
                {
                    p.say("Sprint patterns must be unique.");
                } else {
                    j.sprint_name_patterns.push(pattern);
                }
            }
            1..=3 => {
                let items: Vec<_> = j
                    .sprint_name_patterns
                    .iter()
                    .map(|p| Choice::new(p.as_str(), p.as_str()))
                    .collect();
                let selected = one(
                    p,
                    if action == 3 {
                        "My sprint pattern:"
                    } else {
                        "Sprint pattern:"
                    },
                    &items,
                    j.sprint_name_patterns[0].as_str(),
                )
                .await?;
                let at = j
                    .sprint_name_patterns
                    .iter()
                    .position(|p| p.as_str() == selected)
                    .context("invalid sprint pattern selection")?;
                match action {
                    1 => {
                        let pattern =
                            sprint_pattern_input(p, "Replacement sprint regex:", &selected).await?;
                        if j.sprint_name_patterns
                            .iter()
                            .enumerate()
                            .any(|(i, p)| i != at && p.as_str() == pattern.as_str())
                        {
                            p.say("Sprint patterns must be unique.");
                        } else {
                            j.sprint_name_patterns[at] = pattern;
                        }
                    }
                    2 if j.sprint_name_patterns.len() == 1 => {
                        p.say("Keep at least one sprint pattern.")
                    }
                    2 => {
                        j.sprint_name_patterns.remove(at);
                    }
                    _ => {
                        let primary = j.sprint_name_patterns.remove(at);
                        j.sprint_name_patterns.insert(0, primary);
                    }
                }
            }
            4 => {
                if current != 1 {
                    p.say("Overview will stay empty until you explicitly choose a sprint.");
                    if !p
                        .confirm("Continue with no unique current-sprint default?", false)
                        .await?
                    {
                        supplied = false;
                        args.sprint_name_patterns.clear();
                        edit_primary = true;
                        continue;
                    }
                }
                return Ok(());
            }
            _ => anyhow::bail!("invalid sprint pattern action"),
        }
    }
}

async fn load_board_teams(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &JiraConfig,
    metadata: &Metadata,
    all: &[Choice],
) -> Result<Option<Vec<Choice>>> {
    p.say("Discovering teams from issues on the selected board in ALL returned active sprints (not regex-filtered; no future/history scan)…");
    match cancellable(cancel, d.board_teams(j, metadata)).await {
        Ok(found) => {
            if found.unverified_values > 0 {
                p.say(&format!("{} observed team option(s) did not match selected-project/type create metadata and are unavailable.", found.unverified_values));
            }
            let mut items: Vec<_> = all
                .iter()
                .filter(|choice| found.teams.contains(&choice.id))
                .cloned()
                .collect();
            if items.is_empty() {
                p.say("No verified teams were observed in the selected board's active-sprint issues. Choose another team discovery action; no broader scan runs automatically.");
            }
            if let Some(saved) = all.iter().find(|choice| choice.id == j.team)
                && !items.iter().any(|choice| choice.id == saved.id)
            {
                items.push(Choice::new(
                    &saved.id,
                    format!(
                        "{} (saved; verified create option, not observed in active-sprint issues)",
                        saved.label
                    ),
                ));
            }
            Ok(Some(items))
        }
        Err(error) if error.is::<Cancelled>() => Err(error),
        Err(error) => {
            jira_error(
                p,
                "Selected-board team discovery failed or was incomplete; no partial list is available",
                &error,
                j,
            );
            Ok(None)
        }
    }
}

async fn select_team(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    metadata: &Metadata,
) -> Result<Connected<()>> {
    let all = metadata_teams(j, metadata);
    let mut board = load_board_teams(p, d, cancel, j, metadata, &all).await?;
    let mut next_action = board.as_ref().filter(|items| !items.is_empty()).map(|_| 0);
    p.say("Team choices are verified against selected-project/type create metadata. Ctrl-O opens board choices, all verified metadata choices and retry; no tenant board search is needed.");
    loop {
        let actions = choices(&[
            "Choose from selected-board active-sprint teams and verified saved selection",
            "Choose from ALL verified selected-project/type metadata teams (no scan)",
            "Retry selected-board active-sprint issue discovery",
            "Change Jira connection answers",
            "Skip Jira",
        ]);
        let action = if let Some(action) = next_action.take() {
            action
        } else {
            let default = usize::from(board.as_ref().is_none_or(|items| items.is_empty()));
            p.select("Team actions:", &actions, default).await?
        };
        let items = match action {
            0 => {
                let Some(items) = board.as_ref().filter(|items| !items.is_empty()) else {
                    p.say("No complete verified board team list is available. Retry or explicitly choose all verified metadata teams.");
                    continue;
                };
                items
            }
            1 => &all,
            2 => {
                // Replace, never merge a failed/incomplete attempt with earlier rows.
                board = load_board_teams(p, d, cancel, j, metadata, &all).await?;
                if board.as_ref().is_some_and(|items| !items.is_empty()) {
                    next_action = Some(0);
                }
                continue;
            }
            3 => return Ok(Connected::Change),
            4 => return Ok(Connected::Skip),
            _ => anyhow::bail!("invalid team action"),
        };
        if items.is_empty() {
            p.say("No verified team choices are available for the selected projects/types.");
            continue;
        }
        match p.pick_one("Team:", items, index(items, &j.team)).await {
            Ok(at) => {
                j.team = items.get(at).context("invalid team selection")?.id.clone();
                return Ok(Connected::Ready(()));
            }
            Err(error) if error.is::<OpenMenu>() => {}
            Err(error) => return Err(error),
        }
    }
}

fn index(items: &[Choice], saved: &str) -> usize {
    items.iter().position(|c| c.id == saved).unwrap_or(0)
}

async fn one(p: &mut impl Prompts, message: &str, items: &[Choice], saved: &str) -> Result<String> {
    ensure!(!items.is_empty(), "discovery returned no usable choices");
    if let [only] = items {
        p.say(&format!("{message} {} (only available choice)", only.label));
        return Ok(only.id.clone());
    }
    let i = p.select(message, items, index(items, saved)).await?;
    Ok(items.get(i).context("invalid selection")?.id.clone())
}
async fn text(
    p: &mut impl Prompts,
    message: &str,
    default: &str,
    validate: impl Fn(&str) -> bool,
    help: &str,
) -> Result<String> {
    loop {
        let value = p.text(message, default).await?;
        if validate(&value) {
            return Ok(value);
        }
        p.say(help);
    }
}

fn endpoint(value: &str, http_allowed: bool) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        (u.scheme() == "https" || http_allowed && u.scheme() == "http")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}
fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Enter retains a saved secret, but never silently authorizes a new destination.
pub async fn credential(
    p: &mut impl Prompts,
    saved: Option<&Secret>,
    changed: bool,
) -> Result<Secret> {
    loop {
        let entered = p.password(saved.is_some()).await?;
        if !entered.trim().is_empty() {
            return Ok(Secret::Literal { value: entered });
        }
        if let Some(saved) = saved {
            if !changed
                || p.confirm(
                    "Send the saved credential to the new URL shown above?",
                    false,
                )
                .await?
            {
                return Ok(saved.clone());
            }
            p.say("Reenter a token for this destination, or Ctrl-C to cancel.");
        } else {
            p.say("A token is required. Enter it here; it will never be printed.");
        }
    }
}

pub struct Wizard {
    pub config: Config,
    pub host_port: u16,
    pub channel: Channel,
    cli_url: Option<String>,
    jira_args: JiraArgs,
    nightly_requested: bool,
    identities: BTreeMap<String, String>,
    ai_test_status: &'static str,
}
impl Wizard {
    pub fn new(cli: Cli) -> Self {
        Self {
            config: Config {
                read_only_test_mode: false,
                ..Config::default()
            },
            host_port: 3333,
            channel: if cli.nightly {
                Channel::Nightly
            } else {
                Channel::Stable
            },
            cli_url: cli.jira_cloud_url,
            jira_args: cli.jira,
            nightly_requested: cli.nightly,
            identities: BTreeMap::new(),
            ai_test_status: "not run (optional)",
        }
    }
    pub fn load(&mut self, state: &install::State) -> Result<()> {
        if let Some(yaml) = &state.existing_config {
            self.config = Config::from_yaml(yaml)?;
        }
        self.host_port = state.existing_port.unwrap_or(3333);
        if !self.nightly_requested {
            self.channel = state.existing_channel.unwrap_or(Channel::Stable);
        }
        Ok(())
    }

    pub async fn configure(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.jira(p, d, cancel).await?;
        self.forges(p, d, cancel).await?;
        self.ai(p, d, cancel).await?;
        self.runtime(p).await?;
        self.config.validate()?;
        Ok(())
    }

    async fn remove_jira(&mut self, p: &mut impl Prompts) -> Result<bool> {
        if self.config.jira.is_some() && !p.confirm("Remove Jira and clear forge default-project links from the pending configuration?", false).await? { return Ok(false); }
        self.config.jira = None;
        for f in &mut self.config.forges {
            f.default_project = None;
        }
        p.say("Jira skipped; it will not be included in the saved configuration.");
        Ok(true)
    }

    async fn jira(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
    ) -> Result<()> {
        p.heading("Jira Cloud");
        let mut args = std::mem::take(&mut self.jira_args);
        args.validate()?;
        if !args.any()
            && !p.confirm("Configure Jira Cloud?", true).await?
            && self.remove_jira(p).await?
        {
            return Ok(());
        }
        let mut j = self.config.jira.clone().unwrap_or_else(|| JiraConfig {
            base_url: String::new(),
            email: String::new(),
            token: Secret::Literal {
                value: String::new(),
            },
            projects: vec![],
            board_ids: vec![],
            team: String::new(),
            team_field_name: "AssignedTeam".into(),
            assignee_groups: vec![],
            sprint_name_patterns: vec![],
            defaults: PurposeDefaults::default(),
            project_overrides: BTreeMap::new(),
        });
        if args.any() && self.config.jira.is_some() {
            p.say("Command-line Jira settings propose these changes to the existing configuration; unspecified settings are preserved:");
            for line in jira_args::changes(&args, &j) {
                p.say(&line);
            }
            if !p
                .confirm(
                    "Apply supplied Jira settings to the pending configuration?",
                    false,
                )
                .await?
            {
                args = JiraArgs::default();
            }
        }
        let review_supplied = args.any();
        let mut saved = self
            .config
            .jira
            .as_ref()
            .map(|j| (j.base_url.clone(), j.token.clone()));
        let mut url_default = self.cli_url.clone().unwrap_or_else(|| j.base_url.clone());
        loop {
            j.base_url = if let Some(url) = &args.url {
                p.say(&format!("Jira Cloud tenant URL: {url} (supplied)"));
                url.clone()
            } else {
                text(p, "Jira Cloud tenant URL:", &url_default, |s| jira_cloud_url(s).is_ok(), "Use an HTTPS tenant root such as https://your-team.atlassian.net (no credentials, path, query or fragment).").await?
            };
            j.base_url = jira_cloud_url(&j.base_url).map_err(anyhow::Error::msg)?;
            url_default.clone_from(&j.base_url);
            j.email = text(
                p,
                "Atlassian account email:",
                &j.email,
                |s| !s.trim().is_empty() && s.contains('@'),
                "Enter your Atlassian account email.",
            )
            .await?;
            p.say("Create an UNSCOPED Atlassian API token (not a scoped gateway token). Your account needs project/board/sprint, create-metadata and assignee-group read access. Choose an expiry and renew before it expires.");
            p.say("https://id.atlassian.com/manage-profile/security/api-tokens");
            j.token = credential(
                p,
                saved.as_ref().map(|(_, s)| s),
                saved
                    .as_ref()
                    .is_some_and(|(url, _)| !same_url(url, &j.base_url)),
            )
            .await?;
            saved = Some((j.base_url.clone(), j.token.clone()));
            p.say("Verifying Jira identity (recent projects are non-exhaustive)…");
            let catalog_result = if args.projects.is_some() {
                cancellable(cancel, async {
                    Ok(Catalog {
                        identity: d.identity(&j).await?,
                        ..Catalog::default()
                    })
                })
                .await
            } else {
                cancellable(cancel, d.catalog(&j)).await
            };
            let catalog = match catalog_result {
                Ok(catalog) => catalog,
                Err(error) if error.is::<Cancelled>() => return Err(error),
                Err(error) => {
                    jira_error(
                        p,
                        "Recent project discovery failed; verifying identity separately so manual entry remains available",
                        &error,
                        &j,
                    );
                    match connect(p, cancel, Some(&j), || d.identity(&j)).await? {
                        Connected::Ready(identity) => Catalog {
                            identity,
                            ..Catalog::default()
                        },
                        Connected::Change => {
                            args = JiraArgs::default();
                            continue;
                        }
                        Connected::Skip => {
                            if self.remove_jira(p).await? {
                                return Ok(());
                            }
                            continue;
                        }
                    }
                }
            };
            p.say(&format!(
                "Verified identity: {}. Setup reads do not prove complete runtime API permissions.",
                catalog.identity
            ));
            if !jira_args::projects(p, d, cancel, &mut j, &self.config, &mut args).await? {
                match select_projects(p, d, cancel, &mut j, &self.config, catalog.projects).await? {
                    Connected::Ready(()) => {}
                    Connected::Change => {
                        args = JiraArgs::default();
                        continue;
                    }
                    Connected::Skip => {
                        if self.remove_jira(p).await? {
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            if !jira_args::board(p, d, cancel, &mut j, &mut args).await? {
                match select_board(p, d, cancel, &mut j).await? {
                    Connected::Ready(()) => {}
                    Connected::Change => {
                        args = JiraArgs::default();
                        continue;
                    }
                    Connected::Skip => {
                        if self.remove_jira(p).await? {
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            if !jira_args::groups(p, d, cancel, &mut j, &mut args).await? {
                match select_groups(p, d, cancel, &mut j).await? {
                    Connected::Ready(()) => {}
                    Connected::Change => {
                        args = JiraArgs::default();
                        continue;
                    }
                    Connected::Skip => {
                        if self.remove_jira(p).await? {
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            p.say("Discovering semantic fields after project/board/group selection…");
            let settings = match connect(p, cancel, Some(&j), || async {
                let settings = d.settings(&j).await?;
                ensure!(
                    !team_fields(&settings).is_empty(),
                    "no unique semantic team fields were discovered"
                );
                Ok(settings)
            })
            .await?
            {
                Connected::Ready(settings) => settings,
                Connected::Change => {
                    args = JiraArgs::default();
                    continue;
                }
                Connected::Skip => {
                    if self.remove_jira(p).await? {
                        return Ok(());
                    }
                    continue;
                }
            };
            let mut fields = team_fields(&settings);
            if !jira_args::field(p, d, cancel, &mut j, &mut fields, &mut args).await? {
                // Without a supplied verified name, explicitly ask even for a singleton.
                let field = p
                    .select(
                        "Team field (semantic name):",
                        &fields,
                        index(&fields, &j.team_field_name),
                    )
                    .await?;
                j.team_field_name = fields
                    .get(field)
                    .context("invalid team field selection")?
                    .id
                    .clone();
            }
            p.say("Discovering team options, scoped metadata and active/future sprints…");
            let mut metadata = match connect(p, cancel, Some(&j), || async {
                let m = d.metadata(&j).await?;
                ensure!(
                    m.board_ids == j.board_ids,
                    "metadata does not match selected ordered boards"
                );
                ensure!(
                    m.types.iter().any(|t| !t.teams.is_empty()),
                    "no verified team choices"
                );
                Ok(m)
            })
            .await?
            {
                Connected::Ready(m) => m,
                Connected::Change => {
                    args = JiraArgs::default();
                    continue;
                }
                Connected::Skip => {
                    if self.remove_jira(p).await? {
                        return Ok(());
                    } else {
                        continue;
                    }
                }
            };
            if !jira_args::team(p, d, cancel, &mut j, &mut metadata, &mut args).await? {
                match select_team(p, d, cancel, &mut j, &metadata).await? {
                    Connected::Ready(()) => {}
                    Connected::Change => {
                        args = JiraArgs::default();
                        continue;
                    }
                    Connected::Skip => {
                        if self.remove_jira(p).await? {
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            for t in &metadata.types {
                for note in &t.notes {
                    p.say(&format!("{}/{}: {note}", t.project, t.name));
                }
            }
            select_sprint_patterns(p, &mut j, &metadata, &mut args).await?;
            if review_supplied {
                p.heading("Review Jira settings");
                for line in jira_args::summary(&j) {
                    p.say(&clean(&line));
                }
                if p.confirm("Edit Jira settings before continuing?", false)
                    .await?
                {
                    args = JiraArgs::default();
                    continue;
                }
            }
            let mut candidate = self.config.clone();
            for f in &mut candidate.forges {
                if f.default_project
                    .as_ref()
                    .is_some_and(|key| !j.projects.contains(key))
                {
                    f.default_project = None;
                }
            }
            candidate.jira = Some(j.clone());
            candidate.validate()?;
            self.config = candidate;
            self.identities.insert("jira".into(), catalog.identity);
            return Ok(());
        }
    }

    async fn forges(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
    ) -> Result<()> {
        p.heading("Forge accounts");
        let existing_ids: Vec<_> = self.config.forges.iter().map(|f| f.id.clone()).collect();
        for id in existing_ids {
            let at = self
                .config
                .forges
                .iter()
                .position(|f| f.id == id)
                .context("missing forge")?;
            let existing = self.config.forges[at].clone();
            if !p
                .confirm(
                    &format!(
                        "Configure existing forge {} ({})?",
                        existing.name(),
                        existing.id
                    ),
                    true,
                )
                .await?
                && p.confirm("Remove this forge from the pending configuration?", false)
                    .await?
            {
                self.config.forges.remove(at);
                continue;
            }
            match self.forge(p, d, cancel, Some(existing)).await? {
                Some(f) => self.config.forges[at] = f,
                None => {
                    self.config.forges.remove(at);
                }
            }
        }
        let mut add = p.confirm("Add a forge account?", false).await?;
        while add {
            if self.config.forges.len() >= 64 {
                p.say("The maximum of 64 forge instances has been reached.");
                break;
            }
            if let Some(f) = self.forge(p, d, cancel, None).await? {
                self.config.forges.push(f);
            }
            add = p.confirm("Add another forge?", false).await?;
        }
        Ok(())
    }

    async fn forge(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
        old: Option<ForgeConfig>,
    ) -> Result<Option<ForgeConfig>> {
        p.say("Esc discards this forge draft and returns to forge management; Ctrl-C exits setup.");
        p.local_cancel(true);
        let result = self.forge_draft(p, d, cancel, old.clone()).await;
        p.local_cancel(false);
        match result {
            Err(error) if error.is::<Back>() => {
                p.say("Forge draft discarded; previously configured accounts are unchanged.");
                Ok(old)
            }
            result => result,
        }
    }

    async fn forge_draft(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
        old: Option<ForgeConfig>,
    ) -> Result<Option<ForgeConfig>> {
        let mut saved = old.clone();
        let mut draft = old.clone();
        loop {
            let kinds = [Kind::Github, Kind::Gitlab, Kind::Codeberg, Kind::Forgejo];
            let selected = p
                .select(
                    "Forge provider:",
                    &choices(&["GitHub", "GitLab", "Codeberg", "Forgejo"]),
                    kinds
                        .iter()
                        .position(|k| Some(*k) == draft.as_ref().map(|f| f.kind))
                        .unwrap_or(0),
                )
                .await?;
            let kind = *kinds.get(selected).context("invalid forge provider")?;
            let default_name = draft
                .as_ref()
                .and_then(|f| f.name.clone())
                .unwrap_or_else(|| {
                    default_forge_name(
                        kind,
                        &self.config.forges,
                        old.as_ref().map(|f| f.id.as_str()),
                    )
                });
            let name = text(
                p,
                "Forge name:",
                &default_name,
                |s| !s.trim().is_empty(),
                "Enter a nonempty display name for this forge.",
            )
            .await?
            .trim()
            .to_owned();
            let default_url = draft
                .as_ref()
                .filter(|f| f.kind == kind)
                .map(|f| f.base_url.as_str())
                .unwrap_or(match kind {
                    Kind::Github => "https://api.github.com",
                    Kind::Gitlab => "https://gitlab.com",
                    Kind::Codeberg => "https://codeberg.org",
                    _ => "",
                });
            let base_url = text(
                p,
                "Forge URL (GitHub API base; others instance root):",
                default_url,
                |s| endpoint(s, false),
                "Use HTTPS without credentials, query or fragment.",
            )
            .await?;
            match kind {
                Kind::Github => {
                    p.say("GitHub: create a classic PAT (default). For private repositories, repo is broad and grants write rights PullBoard NEVER uses. Fine-grained PATs are an alternative: grant selected repositories read-only metadata, contents, pull requests and issues; organization approval/SSO may be required. Choose an expiry and renew before it expires.");
                    p.say("https://github.com/settings/tokens/new");
                }
                Kind::Gitlab => {
                    p.say("GitLab: create a personal access token with read_api and access to your private projects. Choose an expiry and renew before it expires.");
                    p.say("Token creation: your GitLab instance → Preferences → Access Tokens. https://docs.gitlab.com/user/profile/personal_access_tokens/");
                }
                _ => {
                    p.say("Forgejo/Codeberg: create an access token in Settings → Applications. Read-only read:user, read:repository and read:issue scopes depend on the deployment; grant private-repository access if needed. Choose an expiry where supported and rotate periodically.");
                    p.say("https://forgejo.org/docs/latest/user/token-scope/");
                }
            }
            let changed = saved
                .as_ref()
                .is_some_and(|f| f.kind != kind || !same_url(&f.base_url, &base_url));
            let token = credential(p, saved.as_ref().map(|f| &f.token), changed).await?;
            let mut f = draft.clone().unwrap_or_else(|| ForgeConfig {
                id: next_source_id(kind, &self.config.forges),
                name: None,
                kind,
                base_url: base_url.clone(),
                token: token.clone(),
                include_repositories: vec![],
                exclude_repositories: vec![],
                default_project: None,
            });
            f.name = Some(name);
            f.kind = kind;
            f.base_url = base_url;
            f.token = token;
            saved = Some(f.clone());
            draft = Some(f.clone());
            p.say("Verifying forge identity…");
            let identity = match connect(p, cancel, None, || d.forge(&f)).await? {
                Connected::Ready(identity) => identity,
                Connected::Change => continue,
                Connected::Skip => {
                    if old.is_none()
                        || p.confirm(
                            "Remove this existing forge from the pending configuration?",
                            false,
                        )
                        .await?
                    {
                        p.say("Forge skipped; it will not be saved.");
                        return Ok(None);
                    } else {
                        continue;
                    }
                }
            };
            p.say(&format!("Verified identity: {identity}. Full repository/PR/issue permissions will be checked during runtime discovery."));
            self.identities.insert(f.id.clone(), identity);
            return Ok(Some(f));
        }
    }

    async fn ai(
        &mut self,
        p: &mut impl Prompts,
        d: &impl Discovery,
        cancel: &CancellationToken,
    ) -> Result<()> {
        p.heading("AI (optional)");
        let old = self.config.ai.clone();
        let selected = p
            .select(
                "AI provider:",
                &choices(&["Disabled", "OpenAI", "Anthropic", "Ollama"]),
                if !old.enabled {
                    0
                } else {
                    match old.provider {
                        AiProvider::Openai => 1,
                        AiProvider::Anthropic => 2,
                        AiProvider::Ollama => 3,
                    }
                },
            )
            .await?;
        if selected == 0 {
            self.config.ai.enabled = false;
            return Ok(());
        }
        p.say("Privacy: selected PR titles, descriptions, diffs and comments — including private repository content — are sent to the configured AI endpoint. PR text is untrusted; AI cannot execute tools. Editable deterministic templates remain available on failure.");
        if !p
            .confirm(
                "Allow selected PR content to be sent to this AI provider?",
                false,
            )
            .await?
        {
            self.config.ai.enabled = false;
            p.say("AI remains disabled; templates are available.");
            return Ok(());
        }
        let provider = match selected {
            1 => AiProvider::Openai,
            2 => AiProvider::Anthropic,
            _ => AiProvider::Ollama,
        };
        let default_endpoint = ai_default_url(provider);
        let same_provider = old.provider.name() == provider.name();
        let base_url = text(p,"AI endpoint:", if same_provider {old.base_url.as_deref().unwrap_or(default_endpoint)} else {default_endpoint}, |s| endpoint(s,matches!(provider,AiProvider::Ollama)), "Use HTTPS for cloud AI; Ollama may use a container-reachable HTTP URL. No credentials, query or fragment in URLs.").await?;
        if matches!(provider, AiProvider::Ollama) {
            p.say("Use an existing container-reachable Ollama server. Setup does not install Ollama or change its listener.");
        }
        let model_default = if same_provider {
            old.model.as_str()
        } else {
            match provider {
                AiProvider::Openai => "gpt-6-luna",
                AiProvider::Anthropic | AiProvider::Ollama => "",
            }
        };
        let model = text(
            p,
            "Exact model ID:",
            model_default,
            |s| !s.trim().is_empty(),
            "Enter the exact model ID available at your endpoint.",
        )
        .await?;
        let token = if matches!(provider, AiProvider::Ollama) {
            None
        } else {
            p.say(match provider {
                AiProvider::Anthropic=>"Create an Anthropic API key with inference access; configure billing/limits and rotate or expire it according to your policy. https://console.anthropic.com/settings/keys",
                _=>"Create an OpenAI API key with model inference access; configure billing/limits and rotate or expire it according to your policy. https://platform.openai.com/api-keys",
            });
            Some(
                credential(
                    p,
                    old.token.as_ref(),
                    !same_provider
                        || !same_url(
                            old.base_url
                                .as_deref()
                                .unwrap_or(ai_default_url(old.provider)),
                            &base_url,
                        ),
                )
                .await?,
            )
        };
        let ai = AiConfig {
            enabled: true,
            provider,
            base_url: Some(base_url),
            model,
            token,
        };
        let mut candidate = self.config.clone();
        candidate.ai = ai.clone();
        candidate.validate()?;
        self.config = candidate;
        if p.confirm("Run an optional synthetic inference test?", false)
            .await?
        {
            p.say("Only synthetic text is sent, never PR content. This request may incur API charges.");
            if p.confirm(
                "Authorize one potentially billable synthetic request?",
                false,
            )
            .await?
            {
                self.ai_test_status = match cancellable(cancel, d.ai_test(&ai)).await {
                    Ok(()) => "succeeded from this host; container connectivity unverified",
                    Err(e) if e.is::<Cancelled>() => return Err(e),
                    Err(_) => "failed; editable deterministic templates remain available",
                };
                p.say(self.ai_test_status);
            }
        }
        Ok(())
    }

    async fn runtime(&mut self, p: &mut impl Prompts) -> Result<()> {
        p.heading("Runtime");
        self.port(p).await?;
        let channel = p
            .select(
                "Container channel:",
                &choices(&["Stable (latest)", "Nightly"]),
                usize::from(self.channel == Channel::Nightly),
            )
            .await?;
        self.channel = if channel == 1 {
            Channel::Nightly
        } else {
            Channel::Stable
        };
        p.say("Test mode simulates Jira writes. Live mode writes only after an explicit Jira submission; forges and setup discovery are always read-only.");
        self.config.read_only_test_mode = p
            .confirm("Start in Test mode?", self.config.read_only_test_mode)
            .await?;
        Ok(())
    }
    async fn port(&mut self, p: &mut impl Prompts) -> Result<()> {
        self.host_port = text(
            p,
            "Local host port:",
            &self.host_port.to_string(),
            |s| s.parse::<u16>().is_ok_and(|n| n >= 1024),
            "Choose a host port between 1024 and 65535.",
        )
        .await?
        .parse()?;
        self.config.server.bind = "0.0.0.0".parse()?;
        self.config.server.port = 8080;
        self.config.server.static_directory = "/app/frontend".into();
        self.config.server.database = "/data/pullboard-cache.sqlite".into();
        self.config.server.allowed_origins = vec![
            format!("http://localhost:{}", self.host_port),
            format!("http://127.0.0.1:{}", self.host_port),
        ];
        Ok(())
    }
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "Dashboard: http://127.0.0.1:{} (loopback only; container port 8080)",
                self.host_port
            ),
            format!("Image: {}", self.channel.image()),
            format!(
                "Jira mode: {}",
                if self.config.read_only_test_mode {
                    "Test (simulated writes)"
                } else {
                    "Live (explicit submissions only)"
                }
            ),
        ];
        if let Some(j) = &self.config.jira {
            lines.extend(jira_args::summary(j));
        } else {
            lines.push("Jira: disabled".into());
        }
        for f in &self.config.forges {
            lines.push(format!(
                "Forge: {} ({}) · {} · {}",
                f.name(),
                f.id,
                kind_name(f.kind),
                f.base_url
            ));
        }
        for (id, identity) in &self.identities {
            lines.push(format!("Verified {id} identity: {identity}"));
        }
        lines.push(if self.config.ai.enabled {
            format!(
                "AI: {} · {} · {} · test {}",
                self.config.ai.provider.name(),
                self.config.ai.model,
                self.config
                    .ai
                    .base_url
                    .as_deref()
                    .unwrap_or("provider default"),
                self.ai_test_status
            )
        } else {
            "AI: disabled (editable templates available)".into()
        });
        lines
    }
    /// Choosing one action is the final explicit authorization, not an extra menu.
    pub async fn authorize(&self, p: &mut impl Prompts) -> Result<bool> {
        self.config.validate()?;
        p.heading("Ready to install");
        for line in self.summary() {
            p.say(&line);
        }
        p.say("Create/replace ~/.config/pullboard/config.yaml and ~/.config/containers/systemd/pullboard.container. Files/backups are private (0600); retain one previous version; YAML comments may change.");
        p.say("Choose an action to authorize these local writes and user-manager reload. Ctrl-C cancels without changes. Install only does not pull or start/restart a service.");
        Ok(p.select(
            "Install:",
            &choices(&["Install and start", "Install only"]),
            0,
        )
        .await?
            == 0)
    }
}

fn team_fields(catalog: &Catalog) -> Vec<Choice> {
    // Duplicate semantic names cannot be selected safely by the runtime.
    let mut counts = BTreeMap::<&str, usize>::new();
    for c in &catalog.fields {
        *counts.entry(&c.id).or_default() += 1;
    }
    catalog
        .fields
        .iter()
        .filter(|c| counts[c.id.as_str()] == 1 && semantic_field(&c.id))
        .cloned()
        .collect()
}
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Github => "github",
        Kind::Gitlab => "gitlab",
        Kind::Codeberg => "codeberg",
        Kind::Forgejo => "forgejo",
        Kind::Jira => "jira",
    }
}
fn default_forge_name(kind: Kind, existing: &[ForgeConfig], current_id: Option<&str>) -> String {
    let ordinal = existing
        .iter()
        .filter(|f| f.kind == kind)
        .take_while(|f| Some(f.id.as_str()) != current_id)
        .count()
        + 1;
    let stem = kind_name(kind);
    (ordinal..)
        .map(|n| {
            if n == 1 {
                stem.to_owned()
            } else {
                format!("{stem}-{n}")
            }
        })
        .find(|name| {
            existing
                .iter()
                .all(|f| Some(f.id.as_str()) == current_id || f.name() != name)
        })
        .expect("finite source list")
}
pub fn next_source_id(kind: Kind, existing: &[ForgeConfig]) -> String {
    let stem = kind_name(kind);
    (1..)
        .map(|n| format!("{stem}-{n}"))
        .find(|id| existing.iter().all(|f| &f.id != id))
        .expect("finite source list")
}
fn ai_default_url(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::Openai => "https://api.openai.com/v1",
        AiProvider::Anthropic => "https://api.anthropic.com/v1",
        AiProvider::Ollama => "http://host.containers.internal:11434",
    }
}

pub fn project_cleanup(config: &Config, selected: &[String]) -> Vec<String> {
    let Some(j) = &config.jira else {
        return vec![];
    };
    let removed: Vec<_> = j
        .projects
        .iter()
        .filter(|s| !selected.contains(s))
        .cloned()
        .collect();
    if removed.is_empty() {
        return vec![];
    }
    let mut lines = vec![format!("Remove Jira projects: {}", removed.join(", "))];
    let overrides: Vec<_> = j
        .project_overrides
        .keys()
        .filter(|s| !selected.contains(s))
        .cloned()
        .collect();
    if !overrides.is_empty() {
        lines.push(format!(
            "Remove orphaned project overrides: {}",
            overrides.join(", ")
        ));
    }
    for f in &config.forges {
        if f.default_project
            .as_ref()
            .is_some_and(|s| !selected.contains(s))
        {
            lines.push(format!(
                "Clear forge {} default project; use primary project instead",
                f.id
            ));
        }
    }
    lines
}

pub async fn run(cli: Cli) -> Result<()> {
    ensure!(
        cfg!(all(target_os = "linux", target_arch = "x86_64")),
        "setup supports Linux x86-64 only"
    );
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "run pullboard-setup in an interactive terminal; credentials are never read from pipes or environment"
    );
    std::panic::set_hook(Box::new(|_| {
        crate::prompt::restore();
        eprintln!(
            "PullBoard setup stopped unexpectedly. Inspect saved files/service status before retrying."
        );
    }));
    let cancel = CancellationToken::new();
    let signals = crate::prompt::cancellation_signals(cancel.clone())?;
    let no_color = cli.no_color || std::env::var_os("NO_COLOR").is_some();
    let mut p = TerminalPrompts::new(no_color, cancel.clone());
    let mut wizard = Wizard::new(cli);
    let result = async {
        p.heading("PullBoard setup"); p.say("One question at a time. Enter accepts defaults; Ctrl-C cancels. No local files change before the final install action.");
        p.heading("Prerequisites");
        let home = std::env::var_os("HOME").context("HOME is unavailable")?;
        let installer = Installer::new()?;
        let state = installer.inspect(Paths::for_home(home.into())?,&cancel).await?;
        wizard.load(&state)?;
        p.say("Rootless Podman, Quadlet and user manager available.");
        if state.existing_config.is_some() { p.say("Existing settings loaded. Enter keeps defaults and saved tokens; YAML-only settings are preserved."); }
        for warning in &state.warnings { p.say(&format!("Warning: {warning}")); }
        let d = LiveDiscovery::new(wizard.config.upstream.clone());
        wizard.configure(&mut p,&d,&cancel).await?;
        loop {
            match installer.check_port(&state,wizard.host_port,&cancel).await {
                Ok(())=>break,
                Err(_) if cancel.is_cancelled()=>return Err(Cancelled.into()),
                Err(e)=>{p.say(&e.to_string()); wizard.port(&mut p).await?;},
            }
        }
        let start = wizard.authorize(&mut p).await?;
        // From here on never race/drop/abort the installer future on cancellation.
        // Its partial report accounts for protected writes and submitted service jobs.
        let yaml = wizard.config.to_yaml()?;
        let report = installer.install(&state,&yaml,wizard.host_port,wizard.channel,start,&cancel, |event| eprintln!("{}",progress(event))).await;
        print_report(&mut p,&report,wizard.config.read_only_test_mode);
        ensure!(report.failure.is_none(),"installation incomplete; see the report above");
        Ok(())
    }.await;
    signals.abort();
    match result {
        Err(e) if e.is::<Cancelled>() || cancel.is_cancelled() => {
            p.say("PullBoard setup cancelled.");
            Ok(())
        }
        other => other,
    }
}

fn progress(event: install::Event) -> &'static str {
    use install::Event::*;
    match event {
        SavingFiles => "Saving protected files…",
        FilesSaved => "Files saved.",
        PullingImage => "Pulling selected image…",
        ImagePulled => "Image pulled.",
        ReloadingManager => "Reloading user manager…",
        StartingService => "Starting service…",
        RestartingService => "Restarting service…",
        WaitingForService => "Waiting for service…",
        ServiceRunning => "Service running.",
        WaitingForHttp => "Waiting for local HTTP…",
        HttpReady => "Local HTTP ready.",
        CheckingSources => "Checking initial source refresh…",
        Sources(_) => "Source status received.",
    }
}
fn print_report(p: &mut impl Prompts, r: &install::InstallReport, test: bool) {
    let mode = if test {
        "Test (simulated)"
    } else {
        "Live (explicit submissions only)"
    };
    if r.failure.is_none()
        && !r.cancelled
        && r.files_saved
        && r.manager_reloaded
        && (!r.service_job_requested || r.service_running && r.http_ready)
    {
        if r.service_job_requested {
            p.heading("PullBoard started.");
            p.emphasize(&r.url);
            p.say(&format!("{mode} · Initial data loads in the background."));
        } else {
            p.heading("PullBoard installed.");
            p.say("Files saved; no service start/restart requested.");
            p.say(&format!("Configured Jira mode: {mode}"));
        }
        for warning in &r.warnings {
            p.say(&format!("Warning: {warning}"));
        }
        return;
    }
    p.heading("Installation report");
    p.say(&format!(
        "Configuration saved: {} · Quadlet saved: {} · user manager reloaded: {}",
        r.config_saved, r.quadlet_saved, r.manager_reloaded
    ));
    p.say(&format!(
        "Service job requested: {} · running: {} · HTTP ready: {}",
        r.service_job_requested, r.service_running, r.http_ready
    ));
    p.say("Dashboard (open when the service is ready):");
    p.emphasize(&r.url);
    if r.cancelled {
        p.say("Cancelled. Any saved files remain; a submitted service job may still finish.");
    }
    if let Some(error) = r.failure {
        p.say(&format!("Failure: {error}"));
    }
    for w in &r.warnings {
        p.say(&format!("Warning: {w}"));
    }
    p.say(&format!("Configured Jira mode: {mode}"));
    p.say("Recovery: systemctl --user status pullboard.service");
    p.say("Logs: journalctl --user -u pullboard.service");
    p.say("After editing YAML: systemctl --user daemon-reload, then systemctl --user restart pullboard.service. Do not enable the generated Quadlet service.");
    p.say("In-app Switch to Live is temporary until restart. For permanent Live, set read_only_test_mode: false in ~/.config/pullboard/config.yaml and restart, or rerun setup.");
}

#[cfg(test)]
mod report_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn jira() -> JiraConfig {
        Config::from_yaml("jira:\n  base_url: https://synthetic.atlassian.net\n  email: human@example.invalid\n  token: {value: synthetic-secret}\n  projects: [ONE]\n  board_ids: [7]\n  team: Team\n  sprint_name_patterns: [Team]\n")
            .unwrap().jira.unwrap()
    }

    #[tokio::test]
    async fn jira_client_reuses_only_one_url_email_and_credential_scope_without_network() {
        let discovery = LiveDiscovery::new(Upstream::default());
        let mut j = jira();
        // Client construction is local only: no identity/GET is executed here.
        let first = discovery.client(&j).await.unwrap();
        j.projects.push("TWO".into());
        j.board_ids = vec![42];
        let same = discovery.client(&j).await.unwrap();
        assert!(Arc::ptr_eq(&first, &same));
        j.email = "other@example.invalid".into();
        let email = discovery.client(&j).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &email));
        j.token = Secret::Literal {
            value: "replacement-synthetic-secret".into(),
        };
        let token = discovery.client(&j).await.unwrap();
        assert!(!Arc::ptr_eq(&email, &token));
        j.base_url = "https://other-synthetic.atlassian.net".into();
        let url = discovery.client(&j).await.unwrap();
        assert!(!Arc::ptr_eq(&token, &url));
        j.base_url = "https://synthetic.atlassian.net".into();
        let returned = discovery.client(&j).await.unwrap();
        assert!(
            !Arc::ptr_eq(&token, &returned),
            "old scopes are not retained"
        );
        j.base_url = "not a URL".into();
        assert!(discovery.client(&j).await.is_err());
        assert!(
            discovery.jira.lock().await.is_none(),
            "failed scope construction invalidates the previous cache"
        );
    }

    #[test]
    fn jira_diagnostics_hide_bodies_and_unsafe_raw_sources_but_keep_safe_causes() {
        let j = jira();
        for body in [
            "<html>synthetic-secret</html>",
            r#"{"error":"synthetic-secret"}"#,
            "Authorization: Basic secret",
        ] {
            let error = anyhow::anyhow!(body).context("recent project lookup");
            let safe = safe_jira_error(&error, &j);
            assert!(safe.contains("recent project lookup"));
            assert!(safe.contains("details withheld"));
            assert!(!safe.contains(body));
            assert!(!safe.contains("synthetic-secret"));
        }
        let error = anyhow::anyhow!("pagination incomplete: max_pages 100 reached")
            .context("selected-project board lookup");
        let safe = safe_jira_error(&error, &j);
        assert!(safe.contains("selected-project board lookup"));
        assert!(safe.contains("pagination incomplete: max_pages 100 reached"));
        let basic = basic_material(b"human@example.invalid:synthetic-secret");
        for encoded in [
            "synthetic-\x1bsecret".to_owned(),
            "%73%79%6e%74%68%65%74%69%63%2D%73%65%63%72%65%74".to_owned(),
            "%2573%2579%256E%2574%2568%2565%2574%2569%2563%252D%2573%2565%2563%2572%2565%2574"
                .to_owned(),
            basic.trim_end_matches('=').to_owned(),
        ] {
            let safe = safe_jira_error(&anyhow::anyhow!("HTTP 403: {encoded}"), &j);
            assert!(safe.contains("HTTP 403"));
            assert!(!safe.contains("synthetic-secret"));
            assert!(!safe.contains(&encoded));
            assert!(safe.contains("[REDACTED]"));
        }
        // A malformed request produces a real reqwest error without any I/O.
        let raw = reqwest::Client::new()
            .get("https://unsafe.invalid/")
            .header("bad header", "synthetic-secret")
            .build()
            .unwrap_err();
        let safe = safe_jira_error(
            &anyhow::Error::new(raw)
                .context("unsafe context https://unsafe.invalid/?token=synthetic-secret"),
            &j,
        );
        assert!(safe.contains("check connectivity"));
        assert!(!safe.contains("unsafe.invalid") && !safe.contains("synthetic-secret"));
    }
}
