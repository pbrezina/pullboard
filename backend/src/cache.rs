//! One-row SQLite snapshots: a publication is a single atomic transaction.
use crate::{
    config::Config,
    error::{Error, Result},
    model::*,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, str::FromStr};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceData {
    #[serde(default)]
    pub jira_choices: Option<crate::jira::Metadata>,
    pub issues: Vec<JiraIssue>,
    pub pull_requests: Vec<PullRequest>,
    #[serde(default)]
    pub upstream_issues: Vec<UpstreamIssue>,
    pub sprints: Vec<Sprint>,
    pub identity: Option<Identity>,
    pub coverage_notes: Vec<String>,
    pub issue_weblinks: Vec<(String, String)>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub jira_choices: Option<crate::jira::Metadata>,
    pub board: Board,
    pub identities: Vec<Identity>,
    pub issue_weblinks: Vec<(String, String)>,
    #[serde(default)]
    pub source_accounts: std::collections::BTreeMap<String, String>,
}
pub struct SourceUpdate {
    pub source_id: String,
    pub result: Result<SourceData>,
}
pub struct Cache {
    pool: SqlitePool,
    writer: Mutex<()>,
    quarantine: Mutex<Snapshot>,
    config: Config,
    pub(crate) resolutions: crate::upstream::Resolutions,
}

impl Cache {
    pub async fn open(path: &Path, config: &Config) -> anyhow::Result<Self> {
        // Private upstream content belongs to the local user, including the cache.
        #[cfg(unix)]
        if path != Path::new(":memory:") {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)
                .await?;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        let options = if path == Path::new(":memory:") {
            SqliteConnectOptions::from_str("sqlite::memory:")?
        } else {
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(if path == Path::new(":memory:") { 1 } else { 5 })
            .connect_with(options.busy_timeout(std::time::Duration::from_secs(10)))
            .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS pullboard_snapshot_v1 (id INTEGER PRIMARY KEY CHECK (id=1), snapshot TEXT NOT NULL)").execute(&pool).await?;
        let cache = Self {
            pool,
            writer: Mutex::new(()),
            quarantine: Mutex::new(Snapshot::default()),
            config: config.clone(),
            resolutions: crate::upstream::Resolutions::default(),
        };
        *cache.quarantine.lock().await = cache.read().await.unwrap_or_default();
        let mut snapshot = Snapshot::default();
        let previous_revision = cache.quarantine.lock().await.board.revision;
        if previous_revision > 0 {
            snapshot.board.revision = previous_revision.saturating_add(1);
        }
        snapshot.board.sources = cache.configured_sources();
        cache
            .store(&snapshot)
            .await
            .map_err(|_| anyhow::anyhow!("could not initialize ephemeral cache"))?;
        Ok(cache)
    }
    pub fn configured_sources(&self) -> Vec<SourceHealth> {
        let mut sources = Vec::new();
        if self.config.jira.is_some() {
            sources.push(source("jira", "Jira", Kind::Jira));
        }
        for f in &self.config.forges {
            sources.push(source(&f.id, f.name(), f.kind));
        }
        sources
    }
    /// Publish a principal before any other source read. Old private data is restored
    /// only after an exact principal/service/policy scope match, never on a read failure.
    pub async fn verify(&self, identity: Identity) -> Result<()> {
        if identity.account_id.is_empty() {
            return Err(Error::upstream());
        }
        let _guard = self.writer.lock().await;
        let id = &identity.source_id;
        let scope = self.scope(&identity)?;
        let mut snapshot = self.read().await?;
        // Reverification of an unchanged Jira principal is not a publication.
        // Keep its identity/choices/cards/revision coherent until the full source
        // transaction commits; only bootstrap or quarantine may publish early.
        if id == "jira"
            && snapshot.source_accounts.get(id) == Some(&scope)
            && snapshot.identities.iter().any(|i| {
                i.source_id == *id && i.kind == identity.kind && i.account_id == identity.account_id
            })
        {
            return Ok(());
        }
        let mut old = self.quarantine.lock().await;
        let changed = snapshot
            .source_accounts
            .get(id)
            .is_some_and(|s| s != &scope);
        if changed {
            self.resolutions.invalidate_source(id);
            snapshot.board.issues.retain(|i| &i.source_id != id);
            snapshot.board.pull_requests.retain(|p| &p.source_id != id);
            snapshot
                .board
                .upstream_issues
                .retain(|i| &i.source_id != id);
            if id == "jira" {
                snapshot.jira_choices = None;
                snapshot.board.sprints.clear();
                snapshot.issue_weblinks.clear();
            }
            if let Some(health) = snapshot
                .board
                .sources
                .iter_mut()
                .find(|h| &h.source_id == id)
            {
                *health = source(id, &health.name, health.kind);
            }
        }
        if !snapshot.source_accounts.contains_key(id) && old.source_accounts.get(id) == Some(&scope)
        {
            snapshot.board.generated_at = old.board.generated_at;
            snapshot.board.issues.extend(
                old.board
                    .issues
                    .iter()
                    .filter(|i| &i.source_id == id)
                    .cloned(),
            );
            snapshot.board.pull_requests.extend(
                old.board
                    .pull_requests
                    .iter()
                    .filter(|p| &p.source_id == id)
                    .cloned(),
            );
            snapshot.board.upstream_issues.extend(
                old.board
                    .upstream_issues
                    .iter()
                    .filter(|i| &i.source_id == id)
                    .cloned(),
            );
            if id == "jira" {
                snapshot.jira_choices = old.jira_choices.clone();
                snapshot.board.sprints = old.board.sprints.clone();
                snapshot.issue_weblinks = old.issue_weblinks.clone();
            }
            if let (Some(h), Some(previous)) = (
                snapshot
                    .board
                    .sources
                    .iter_mut()
                    .find(|s| &s.source_id == id),
                old.board.sources.iter().find(|s| &s.source_id == id),
            ) {
                h.last_success_at = previous.last_success_at;
                h.coverage_notes = previous.coverage_notes.clone();
                if h.last_success_at.is_some() {
                    h.status = "error".into();
                    h.error = Some("Cached data restored; awaiting a successful refresh.".into());
                }
            }
        }
        old.board.issues.retain(|i| &i.source_id != id);
        old.board.pull_requests.retain(|p| &p.source_id != id);
        old.board.upstream_issues.retain(|i| &i.source_id != id);
        old.identities.retain(|i| &i.source_id != id);
        old.source_accounts.remove(id);
        if id == "jira" {
            old.jira_choices = None;
            old.board.sprints.clear();
            old.issue_weblinks.clear();
        }
        snapshot.source_accounts.insert(id.clone(), scope);
        snapshot.identities.retain(|i| &i.source_id != id);
        snapshot.identities.push(identity);
        reconcile_links(&mut snapshot);
        snapshot.board.revision = snapshot.board.revision.saturating_add(1);
        self.store(&snapshot).await
    }
    fn scope(&self, i: &Identity) -> Result<String> {
        let (base, policy) = if i.kind == Kind::Jira && i.source_id == "jira" {
            let j = self.config.jira.as_ref().ok_or_else(Error::internal)?;
            (
                &j.base_url,
                // Effective policy lives inside the committed complete generation.
                // New configuration cannot partially replace it before refresh succeeds.
                format!("targeted-jql-v1:choices-v1:{:?}:{}", j.projects, j.board_id),
            )
        } else {
            let f = self
                .config
                .forges
                .iter()
                .find(|f| f.id == i.source_id && f.kind == i.kind)
                .ok_or_else(Error::internal)?;
            (
                &f.base_url,
                format!("{:?}:{:?}", f.include_repositories, f.exclude_repositories),
            )
        };
        let url = url::Url::parse(base).map_err(|_| Error::internal())?;
        Ok(format!(
            "v2:{}:{:?}:{}:{}:{policy}",
            i.source_id,
            i.kind,
            url.as_str().trim_end_matches('/'),
            i.account_id
        ))
    }
    pub async fn read(&self) -> Result<Snapshot> {
        let data: Option<String> =
            sqlx::query_scalar("SELECT snapshot FROM pullboard_snapshot_v1 WHERE id=1")
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| Error::internal())?;
        match data {
            Some(json) => serde_json::from_str(&json).map_err(|_| Error::internal()),
            None => Ok(Snapshot::default()),
        }
    }
    pub(crate) async fn store_resolution(
        &self,
        expected_scope: &str,
        issue: UpstreamIssue,
    ) -> Result<()> {
        let _guard = self.writer.lock().await;
        let snapshot = self.read().await?;
        if snapshot
            .source_accounts
            .get(&issue.source_id)
            .map(String::as_str)
            != Some(expected_scope)
        {
            return Err(crate::upstream::not_ready());
        }
        self.resolutions.insert(expected_scope.into(), issue)
    }
    async fn store(&self, snapshot: &Snapshot) -> Result<()> {
        let json = serde_json::to_string(snapshot).map_err(|_| Error::internal())?;
        let mut tx = self.pool.begin().await.map_err(|_| Error::internal())?;
        sqlx::query("INSERT INTO pullboard_snapshot_v1(id,snapshot) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET snapshot=excluded.snapshot").bind(json).execute(&mut *tx).await.map_err(|_|Error::internal())?;
        tx.commit().await.map_err(|_| Error::internal())
    }
    pub async fn publish(&self, updates: Vec<SourceUpdate>) -> Result<Snapshot> {
        self.publish_attempts(updates, &[]).await
    }
    pub(crate) async fn publish_attempts(
        &self,
        updates: Vec<SourceUpdate>,
        live_health: &[SourceHealth],
    ) -> Result<Snapshot> {
        let _guard = self.writer.lock().await;
        let mut snapshot = self.read().await?;
        // Include skipped-backoff and prior publication-failure health, which may
        // be newer than the still-visible board. No successful timestamp is added
        // until this transaction actually commits.
        for health in &mut snapshot.board.sources {
            if let Some(live) = live_health.iter().find(|s| s.source_id == health.source_id) {
                *health = live.clone();
            }
        }
        let now = Utc::now();
        for update in updates {
            let result = match update.result {
                Ok(data)
                    if !snapshot.source_accounts.contains_key(&update.source_id)
                        || data.identity.as_ref().is_some_and(|identity| {
                            !snapshot.identities.iter().any(|i| {
                                i.source_id == identity.source_id
                                    && i.kind == identity.kind
                                    && i.account_id == identity.account_id
                            })
                        }) =>
                {
                    Err(Error::unavailable())
                }
                result => result,
            };
            let Some(health) = snapshot
                .board
                .sources
                .iter_mut()
                .find(|s| s.source_id == update.source_id)
            else {
                continue;
            };
            if live_health.is_empty() {
                health.last_attempt_at = Some(now);
            }
            match result {
                Ok(data) => {
                    snapshot
                        .board
                        .issues
                        .retain(|i| i.source_id != update.source_id);
                    snapshot
                        .board
                        .pull_requests
                        .retain(|p| p.source_id != update.source_id);
                    snapshot.board.issues.extend(data.issues);
                    snapshot.board.pull_requests.extend(data.pull_requests);
                    snapshot
                        .board
                        .upstream_issues
                        .retain(|i| i.source_id != update.source_id);
                    snapshot.board.upstream_issues.extend(data.upstream_issues);
                    if update.source_id == "jira" {
                        snapshot.jira_choices = data.jira_choices;
                        snapshot.board.sprints = data.sprints;
                        snapshot.issue_weblinks = data.issue_weblinks;
                    }
                    if let Some(identity) = data.identity {
                        snapshot
                            .identities
                            .retain(|i| i.source_id != update.source_id);
                        snapshot.identities.push(identity);
                    }
                    health.status = "ok".into();
                    health.last_success_at = Some(now);
                    health.error = None;
                    health.retry_at = None;
                    health.coverage_notes = data.coverage_notes;
                    health.coverage_notes.sort();
                    health.coverage_notes.dedup();
                }
                Err(error) => {
                    health.status = if error.retry_at.is_some() {
                        "backoff"
                    } else {
                        "error"
                    }
                    .into();
                    health.error = Some(error.message.into());
                    health.retry_at = error.retry_at;
                }
            }
        }
        reconcile_links(&mut snapshot);
        snapshot.board.issues.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        snapshot.board.pull_requests.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        snapshot.board.sprints.sort_by(|a, b| {
            a.team
                .cmp(&b.team)
                .then_with(|| a.board_id.cmp(&b.board_id))
                .then_with(|| a.id.cmp(&b.id))
        });
        snapshot.board.upstream_issues.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        snapshot.board.revision = snapshot.board.revision.saturating_add(1);
        snapshot.board.generated_at = Some(now);
        self.store(&snapshot).await?;
        Ok(snapshot)
    }
}
fn reconcile_links(snapshot: &mut Snapshot) {
    // Cross-source links are derived, not durable authorizations to retain another
    // account's private issue keys or PR IDs after a principal is quarantined.
    for pr in &mut snapshot.board.pull_requests {
        pr.linked_issue_keys =
            crate::forge::references(&format!("{}\n{}", pr.title, pr.description));
        for (key, url) in &snapshot.issue_weblinks {
            if url == &pr.url {
                pr.linked_issue_keys.push(key.clone());
            }
        }
        pr.linked_issue_keys.sort();
        pr.linked_issue_keys.dedup();
    }
    for issue in &mut snapshot.board.issues {
        issue.linked_pr_ids = snapshot
            .board
            .pull_requests
            .iter()
            .filter(|pr| pr.linked_issue_keys.contains(&issue.key))
            .map(|pr| pr.id.clone())
            .collect();
        issue.linked_pr_ids.sort();
        issue.linked_pr_ids.dedup();
    }
}
fn source(id: &str, name: &str, kind: Kind) -> SourceHealth {
    SourceHealth {
        source_id: id.into(),
        name: name.into(),
        kind,
        status: "pending".into(),
        last_success_at: None,
        last_attempt_at: None,
        error: None,
        retry_at: None,
        coverage_notes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn atomic_snapshots_and_retention() {
        let dir = tempfile::tempdir().unwrap();
        let c=Config{jira:Some(serde_yaml::from_str("base_url: https://jira.example\nemail: me@example.com\ntoken: {value: secret}\nprojects: [DEV]\nboard_id: 12\nteam: Platform\nsprint_name_pattern: literal").unwrap()),..Default::default()};
        let cache = std::sync::Arc::new(
            Cache::open(&dir.path().join("cache.sqlite"), &c)
                .await
                .unwrap(),
        );
        cache
            .verify(Identity {
                source_id: "jira".into(),
                kind: Kind::Jira,
                account_id: "me".into(),
                display_name: "Me".into(),
                username: String::new(),
                profile_url: None,
            })
            .await
            .unwrap();
        let writer = cache.clone();
        let task = tokio::spawn(async move {
            for _ in 0..20 {
                writer
                    .publish(vec![SourceUpdate {
                        source_id: "jira".into(),
                        result: Ok(SourceData::default()),
                    }])
                    .await
                    .unwrap();
            }
        });
        for _ in 0..30 {
            let s = cache.read().await.unwrap();
            if s.board.generated_at.is_some() {
                assert!(s.board.generated_at.is_some());
                assert_eq!(s.board.sources[0].status, "ok");
            }
        }
        task.await.unwrap();
        let last = cache.read().await.unwrap().board.sources[0].last_success_at;
        let s = cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: Err(Error::upstream()),
            }])
            .await
            .unwrap();
        assert_eq!(s.board.sources[0].last_success_at, last);
        assert_eq!(s.board.sources[0].status, "error");
        assert_eq!(s.board.revision, 22);
    }
}
