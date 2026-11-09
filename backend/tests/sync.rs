mod common;
use async_trait::async_trait;
use common::*;
use pullboard::{
    cache::{Cache, SourceData, SourceUpdate},
    error::{Error, Result},
    forge::Forge,
    model::{Kind, SourcePhase, SyncStatus},
    sync::{Progress, Syncer},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Barrier, Notify, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

struct ControlledForge {
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
#[async_trait]
impl Forge for ControlledForge {
    fn source_id(&self) -> &str {
        "test-forge"
    }
    async fn identity(&self) -> Result<pullboard::model::Identity> {
        Ok(identity("test-forge", pullboard::model::Kind::Github, "1"))
    }
    async fn discover_with_progress(
        &self,
        progress: pullboard::sync::Progress,
    ) -> Result<SourceData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        progress.fetching(Some(1));
        progress.item_completed();
        Ok(SourceData {
            pull_requests: vec![pr()],
            ..Default::default()
        })
    }
}

struct ConcurrentForge {
    id: String,
    account: String,
    identity_barrier: Option<Arc<Barrier>>,
    entered: mpsc::UnboundedSender<String>,
    release: Arc<Semaphore>,
    failure: Option<Error>,
}
#[async_trait]
impl Forge for ConcurrentForge {
    fn source_id(&self) -> &str {
        &self.id
    }
    async fn identity(&self) -> Result<pullboard::model::Identity> {
        if let Some(barrier) = &self.identity_barrier {
            barrier.wait().await;
        }
        Ok(identity(&self.id, Kind::Github, &self.account))
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        progress.discovering();
        self.entered.send(self.id.clone()).unwrap();
        self.release.acquire().await.unwrap().forget();
        progress.fetching(Some(1));
        progress.item_completed();
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let mut p = pr();
        p.id = format!("{}:acme/widget:42", self.id);
        p.source_id = self.id.clone();
        p.title = format!("New {}", self.id);
        Ok(SourceData {
            pull_requests: vec![p],
            ..Default::default()
        })
    }
}
async fn entered(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn wait_status(sync: &Syncer, ready: impl Fn(&SyncStatus) -> bool) -> SyncStatus {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status = sync.status().await.unwrap();
            if ready(&status) {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn source_work_is_bounded_and_a_later_source_starts_while_earlier_sources_are_blocked() {
    let mut c = config();
    c.forges = (0..5)
        .map(|n| {
            let mut f = forge("github", "https://forge.example");
            f.id = format!("source-{n}");
            f
        })
        .collect();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let gates: Vec<_> = (0..5).map(|_| Arc::new(Semaphore::new(0))).collect();
    let adapters = c
        .forges
        .iter()
        .zip(&gates)
        .map(|(f, release)| {
            Arc::new(ConcurrentForge {
                id: f.id.clone(),
                account: "1".into(),
                identity_barrier: None,
                entered: tx.clone(),
                release: release.clone(),
                failure: None,
            }) as Arc<dyn Forge>
        })
        .collect();
    let sync = Syncer::new(cache.clone(), adapters, None, 3600);
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    let mut started = Vec::new();
    for _ in 0..4 {
        started.push(entered(&mut rx).await);
    }
    started.sort();
    assert_eq!(started, ["source-0", "source-1", "source-2", "source-3"]);
    let status = sync.status().await.unwrap();
    assert_eq!(status.progress[4].phase, SourcePhase::Queued);
    assert!(status.progress[4].started_at.is_none());
    assert_eq!(status.progress[4].elapsed_ms, 0);
    // Complete only one slot. The first three remain blocked throughout.
    gates[3].add_permits(1);
    assert_eq!(entered(&mut rx).await, "source-4");
    let waiting = sync.status().await.unwrap();
    assert_eq!(waiting.progress[3].phase, SourcePhase::Publishing);
    assert!(
        waiting.progress[..3]
            .iter()
            .all(|p| p.phase == SourcePhase::Discovering)
    );
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
    let revision = cache.read().await.unwrap().board.revision;
    for gate in &gates {
        gate.add_permits(1);
    }
    let completed = wait_status(&sync, |s| s.state == "idle").await;
    assert!(completed.last_success_at.is_some());
    let board = cache.read().await.unwrap().board;
    assert_eq!(board.revision, revision + 1);
    assert_eq!(board.pull_requests.len(), 5);
    assert_eq!(
        completed
            .sources
            .iter()
            .map(|s| &s.source_id)
            .collect::<Vec<_>>(),
        c.forges.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn parallel_source_failure_is_isolated_and_all_results_publish_once_after_the_blocked_source()
{
    let mut c = config();
    c.forges = ["blocked", "failed", "ready"]
        .map(|id| {
            let mut f = forge("github", "https://forge.example");
            f.id = id.into();
            f
        })
        .into();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    verify_sources(&cache, &c).await;
    let mut updates = Vec::new();
    for f in &c.forges {
        let mut p = pr();
        p.source_id = f.id.clone();
        p.id = format!("{}:acme/widget:42", f.id);
        p.title = format!("Old {}", f.id);
        updates.push(SourceUpdate {
            source_id: f.id.clone(),
            result: Ok(SourceData {
                pull_requests: vec![p],
                ..Default::default()
            }),
        });
    }
    let old = cache.publish(updates).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let gates: Vec<_> = (0..3).map(|_| Arc::new(Semaphore::new(0))).collect();
    let adapters = c
        .forges
        .iter()
        .zip(&gates)
        .map(|(f, release)| {
            Arc::new(ConcurrentForge {
                id: f.id.clone(),
                account: "1".into(),
                identity_barrier: None,
                entered: tx.clone(),
                release: release.clone(),
                failure: (f.id == "failed").then(Error::upstream),
            }) as Arc<dyn Forge>
        })
        .collect();
    let sync = Syncer::new(cache.clone(), adapters, None, 3600);
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    for _ in 0..3 {
        entered(&mut rx).await;
    }
    let before = cache.read().await.unwrap().board;
    gates[1].add_permits(1);
    let failure = wait_status(&sync, |s| s.progress[1].phase == SourcePhase::Failed).await;
    gates[2].add_permits(1);
    let waiting = wait_status(&sync, |s| s.progress[2].phase == SourcePhase::Publishing).await;
    assert_eq!(waiting.progress[0].phase, SourcePhase::Discovering);
    assert_eq!(
        waiting.progress[1].elapsed_ms,
        failure.progress[1].elapsed_ms
    );
    assert_eq!(waiting.sources[1].status, "error");
    assert_eq!(
        waiting.sources[1].last_success_at,
        old.board.sources[1].last_success_at
    );
    assert_eq!(
        serde_json::to_value(cache.read().await.unwrap().board).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    gates[0].add_permits(1);
    let completed = wait_status(&sync, |s| s.state == "idle").await;
    assert!(completed.last_success_at.is_none());
    assert_eq!(completed.progress[0].phase, SourcePhase::Completed);
    assert_eq!(completed.progress[1].phase, SourcePhase::Failed);
    assert_eq!(completed.progress[2].phase, SourcePhase::Completed);
    let board = cache.read().await.unwrap().board;
    assert_eq!(board.revision, before.revision + 1);
    for (id, title) in [
        ("blocked", "New blocked"),
        ("failed", "Old failed"),
        ("ready", "New ready"),
    ] {
        assert_eq!(
            board
                .pull_requests
                .iter()
                .find(|p| p.source_id == id)
                .unwrap()
                .title,
            title
        );
    }
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn simultaneous_identity_verification_keeps_each_sources_quarantine_and_health_scope_separate()
 {
    let mut c = config();
    c.forges = ["changed", "unchanged"]
        .map(|id| {
            let mut f = forge("github", "https://forge.example");
            f.id = id.into();
            f
        })
        .into();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    verify_sources(&cache, &c).await;
    let mut updates = Vec::new();
    for f in &c.forges {
        let mut p = pr();
        p.source_id = f.id.clone();
        p.id = format!("{}:acme/widget:42", f.id);
        updates.push(SourceUpdate {
            source_id: f.id.clone(),
            result: Ok(SourceData {
                pull_requests: vec![p],
                coverage_notes: vec![f.id.clone()],
                ..Default::default()
            }),
        });
    }
    let old = cache.publish(updates).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let barrier = Arc::new(Barrier::new(2));
    let release = Arc::new(Semaphore::new(0));
    let adapters = c
        .forges
        .iter()
        .map(|f| {
            Arc::new(ConcurrentForge {
                id: f.id.clone(),
                account: if f.id == "changed" { "2" } else { "1" }.into(),
                identity_barrier: Some(barrier.clone()),
                entered: tx.clone(),
                release: release.clone(),
                failure: Some(Error::upstream()),
            }) as Arc<dyn Forge>
        })
        .collect();
    let sync = Syncer::new(cache.clone(), adapters, None, 3600);
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    entered(&mut rx).await;
    entered(&mut rx).await;
    let active = sync.status().await.unwrap();
    assert!(active.sources[0].last_success_at.is_none());
    assert!(active.sources[0].coverage_notes.is_empty());
    assert_eq!(
        active.sources[1].last_success_at,
        old.board.sources[1].last_success_at
    );
    assert_eq!(active.sources[1].coverage_notes, vec!["unchanged"]);
    let verified = cache.read().await.unwrap();
    assert_eq!(verified.board.pull_requests.len(), 1);
    assert_eq!(verified.board.pull_requests[0].source_id, "unchanged");
    for (id, account) in [("changed", "2"), ("unchanged", "1")] {
        assert_eq!(
            verified
                .identities
                .iter()
                .find(|i| i.source_id == id)
                .unwrap()
                .account_id,
            account
        );
    }
    release.add_permits(2);
    let failed = wait_status(&sync, |s| s.state == "idle").await;
    assert!(failed.sources.iter().all(|s| s.status == "error"));
    assert!(failed.sources[0].last_success_at.is_none());
    assert_eq!(
        failed.sources[1].last_success_at,
        old.board.sources[1].last_success_at
    );
    assert_eq!(cache.read().await.unwrap().board.pull_requests.len(), 1);
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn forge_finishes_reading_while_jira_identity_is_blocked_without_publishing_early() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let jira_started = Arc::new(Semaphore::new(0));
    let jira_release = Arc::new(Semaphore::new(0));
    let started = jira_started.clone();
    let release = jira_release.clone();
    let router = axum::Router::new().route(
        "/rest/api/3/myself",
        axum::routing::get(move || {
            let started = started.clone();
            let release = release.clone();
            async move {
                started.add_permits(1);
                // Once opened, allow any subsequent identity revalidation too.
                let _permit = release.acquire().await.unwrap();
                axum::Json(jira_user("me"))
            }
        }),
    );
    let mut c = config();
    c.jira = Some(jira(&format!("http://{}", listener.local_addr().unwrap())));
    c.forges = vec![forge("github", "https://forge.example")];
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let jira = pullboard::jira::Jira::new(c.jira.as_ref().unwrap(), &c.upstream)
        .await
        .unwrap();
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let forge_release = Arc::new(Semaphore::new(0));
    let adapter = Arc::new(ConcurrentForge {
        id: "test-forge".into(),
        account: "1".into(),
        identity_barrier: None,
        entered: tx,
        release: forge_release.clone(),
        failure: None,
    });
    let sync = Syncer::new(cache.clone(), vec![adapter], Some(jira), 3600);
    let cancel = CancellationToken::new();
    let worker = sync.spawn(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), jira_started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(entered(&mut rx).await, "test-forge");
    let before = cache.read().await.unwrap().board;
    forge_release.add_permits(1);
    let waiting = wait_status(&sync, |s| s.progress[1].phase == SourcePhase::Publishing).await;
    assert_eq!(waiting.progress[0].phase, SourcePhase::Preparing);
    assert!(waiting.progress.iter().all(|p| p.finished_at.is_none()));
    assert!(waiting.sources[1].last_success_at.is_none());
    assert_eq!(cache.read().await.unwrap().board.revision, before.revision);
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
    // Metadata is deliberately absent: Jira fails independently after its identity
    // is released, while the completed forge data still publishes successfully.
    jira_release.add_permits(1);
    let completed = wait_status(&sync, |s| s.state == "idle").await;
    assert_eq!(completed.sources[0].status, "error");
    assert_eq!(completed.sources[1].status, "ok");
    assert_eq!(completed.progress[0].phase, SourcePhase::Failed);
    assert_eq!(completed.progress[1].phase, SourcePhase::Completed);
    assert!(completed.last_success_at.is_none());
    assert_eq!(cache.read().await.unwrap().board.pull_requests.len(), 1);
    cancel.cancel();
    worker.await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn startup_manual_refreshes_coalesce_without_overlap() {
    let mut c = config();
    c.forges = vec![forge("github", "https://api.github.com")];
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let f = Arc::new(ControlledForge {
        calls: calls.clone(),
        entered: entered.clone(),
        release: release.clone(),
    });
    let sync = Syncer::new(cache.clone(), vec![f], None, 3600);
    let cancel = CancellationToken::new();
    let handle = sync.spawn(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    for _ in 0..20 {
        assert_eq!(sync.request().await.unwrap().state, "syncing");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if sync.status().await.unwrap().state == "idle" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.read().await.unwrap().board.pull_requests.len(), 1);
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn successful_write_during_sync_schedules_one_nonoverlapping_followup() {
    let mut c = config();
    c.forges = vec![forge("github", "https://api.github.com")];
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let f = Arc::new(ControlledForge {
        calls: calls.clone(),
        entered: entered.clone(),
        release: release.clone(),
    });
    let sync = Syncer::new(cache.clone(), vec![f], None, 3600);
    let cancel = CancellationToken::new();
    let handle = sync.spawn(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    for _ in 0..10 {
        sync.after_write().await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if sync.status().await.unwrap().state == "idle" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cache.read().await.unwrap().board.revision, 4);
    cancel.cancel();
    handle.await.unwrap();
}
#[tokio::test]
async fn failed_source_retains_actual_items_and_last_success_while_other_source_updates() {
    let mut c = config();
    c.forges = vec![forge("github", "https://api.github.com")];
    c.jira = Some(jira("https://jira.example"));
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    let first = cache
        .publish(vec![
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![issue()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    let success = first
        .board
        .sources
        .iter()
        .find(|s| s.source_id == "test-forge")
        .unwrap()
        .last_success_at;
    let second = cache
        .publish(vec![
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Err(Error::upstream()),
            },
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData::default()),
            },
        ])
        .await
        .unwrap();
    assert!(second.board.issues.is_empty());
    assert_eq!(second.board.pull_requests.len(), 1);
    let source = second
        .board
        .sources
        .iter()
        .find(|s| s.source_id == "test-forge")
        .unwrap();
    assert_eq!(source.status, "error");
    assert_eq!(source.last_success_at, success);
    assert!(source.error.is_some());
}
#[tokio::test]
async fn cache_reopen_preserves_snapshots_but_does_not_expose_unconfigured_sources() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("cache.sqlite");
    c.forges = vec![forge("github", "https://api.github.com")];
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    cache
        .verify(identity("test-forge", pullboard::model::Kind::Github, "1"))
        .await
        .unwrap();
    cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                pull_requests: vec![pr()],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    drop(cache);
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
    assert!(cache.read().await.unwrap().identities.is_empty());
    cache
        .verify(identity("test-forge", pullboard::model::Kind::Github, "1"))
        .await
        .unwrap();
    assert_eq!(cache.read().await.unwrap().board.pull_requests.len(), 1);
    drop(cache);
    c.forges.clear();
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    let s = cache.read().await.unwrap();
    assert!(s.board.pull_requests.is_empty());
    assert!(s.board.sources.is_empty());
}

#[tokio::test]
async fn changing_identity_with_same_source_id_never_reuses_private_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("cache.sqlite");
    c.forges = vec![forge("github", "https://api.github.com")];
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Ok(SourceData {
                pull_requests: vec![pr()],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    drop(cache);
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    cache
        .verify(identity(
            "test-forge",
            pullboard::model::Kind::Github,
            "different",
        ))
        .await
        .unwrap();
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
    assert_eq!(
        cache.read().await.unwrap().identities[0].account_id,
        "different"
    );
    cache
        .publish(vec![SourceUpdate {
            source_id: "test-forge".into(),
            result: Err(Error::upstream()),
        }])
        .await
        .unwrap();
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
}

#[tokio::test]
async fn failed_bootstrap_keeps_private_snapshot_and_identity_quarantined() {
    use pullboard::model::Kind;
    let dir = tempfile::tempdir().unwrap();
    let server = wiremock::MockServer::start().await;
    let mut c = config();
    c.server.database = dir.path().join("bootstrap.sqlite");
    c.jira = Some(jira(&server.uri()));
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    cache
        .verify(identity("jira", Kind::Jira, "old"))
        .await
        .unwrap();
    cache
        .publish(vec![SourceUpdate {
            source_id: "jira".into(),
            result: Ok(SourceData {
                issues: vec![issue()],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    drop(cache);
    let app = pullboard::api::App::build(c).await.unwrap();
    let before = app.cache.read().await.unwrap();
    assert!(before.board.issues.is_empty());
    assert!(before.identities.is_empty());
    let cancel = CancellationToken::new();
    let handle = app.sync.spawn(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if app.cache.read().await.unwrap().board.sources[0]
                .last_attempt_at
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    handle.await.unwrap();
    let snapshot = app.cache.read().await.unwrap();
    assert!(snapshot.board.issues.is_empty());
    assert!(snapshot.identities.is_empty());
    assert_eq!(snapshot.board.sources[0].status, "error");
}

#[tokio::test]
async fn jira_identity_is_published_and_changed_account_purged_even_when_metadata_refresh_fails() {
    use pullboard::model::Kind;
    let dir = tempfile::tempdir().unwrap();
    let server = wiremock::MockServer::start().await;
    mock_get(&server, "/rest/api/3/myself", jira_user("new-account")).await;
    let mut c = config();
    c.server.database = dir.path().join("independent.sqlite");
    c.jira = Some(jira(&server.uri()));
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    cache
        .verify(identity("jira", Kind::Jira, "old-account"))
        .await
        .unwrap();
    cache
        .publish(vec![SourceUpdate {
            source_id: "jira".into(),
            result: Ok(SourceData {
                issues: vec![issue()],
                issue_weblinks: vec![("DEV-99".into(), pr().url)],
                ..Default::default()
            }),
        }])
        .await
        .unwrap();
    drop(cache);
    let app = pullboard::api::App::build(c).await.unwrap();
    let cancel = CancellationToken::new();
    let handle = app.sync.spawn(cancel.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if app.cache.read().await.unwrap().board.sources[0]
                .last_attempt_at
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    handle.await.unwrap();
    let snapshot = app.cache.read().await.unwrap();
    assert_eq!(snapshot.identities[0].account_id, "new-account");
    assert!(snapshot.board.issues.is_empty());
    assert!(snapshot.issue_weblinks.is_empty());
    assert_eq!(snapshot.board.sources[0].status, "error");
    assert!(app.jira.as_ref().unwrap().projects().await.is_empty());
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
async fn same_verified_account_restores_last_good_stale_snapshot_but_service_change_does_not() {
    use pullboard::model::Kind;
    for service_change in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut c = config();
        c.server.database = dir.path().join("same.sqlite");
        c.jira = Some(jira("https://jira.example"));
        let cache = Cache::open(&c.server.database, &c).await.unwrap();
        cache
            .verify(identity("jira", Kind::Jira, "me"))
            .await
            .unwrap();
        cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![issue()],
                    ..Default::default()
                }),
            }])
            .await
            .unwrap();
        let success = cache.read().await.unwrap().board.sources[0].last_success_at;
        drop(cache);
        c.jira.as_mut().unwrap().token = pullboard::config::Secret::Literal {
            value: "changed-credential-not-a-cache-key".into(),
        };
        if service_change {
            c.jira.as_mut().unwrap().base_url = "https://other.example".into();
        }
        let cache = Cache::open(&c.server.database, &c).await.unwrap();
        assert!(cache.read().await.unwrap().board.issues.is_empty());
        cache
            .verify(identity("jira", Kind::Jira, "me"))
            .await
            .unwrap();
        let s = cache
            .publish(vec![SourceUpdate {
                source_id: "jira".into(),
                result: Err(Error::upstream()),
            }])
            .await
            .unwrap();
        assert_eq!(s.board.issues.len(), usize::from(!service_change));
        assert_eq!(
            s.board.sources[0].last_success_at,
            if service_change { None } else { success }
        );
        assert_eq!(s.board.sources[0].status, "error");
        assert!(
            !serde_json::to_string(&s.source_accounts)
                .unwrap()
                .contains("changed-credential")
        );
    }
}

#[tokio::test]
async fn legacy_broad_jira_discovery_scope_is_not_restored_even_for_the_same_principal() {
    use pullboard::model::Kind;
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("legacy-jira.sqlite");
    c.jira = Some(jira("https://jira.example"));
    c.forges = vec![forge("github", "https://api.github.com")];
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    let mut snapshot = cache
        .publish(vec![
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![issue()],
                    issue_weblinks: vec![("PRIVATE-99".into(), pr().url)],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    let scope = snapshot.source_accounts.get_mut("jira").unwrap();
    assert!(scope.contains("targeted-jql-v1:"));
    *scope = scope.replacen("targeted-jql-v1:", "", 1);
    drop(cache);
    // Reproduce the exact pre-targeted-discovery policy scope in the ephemeral DB.
    let pool = sqlx::SqlitePool::connect(c.server.database.to_str().unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE pullboard_snapshot_v1 SET snapshot=? WHERE id=1")
        .bind(serde_json::to_string(&snapshot).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    let restored = cache
        .publish(vec![SourceUpdate {
            source_id: "jira".into(),
            result: Err(Error::upstream()),
        }])
        .await
        .unwrap();
    assert!(restored.board.issues.is_empty());
    assert!(restored.board.sprints.is_empty());
    assert!(restored.issue_weblinks.is_empty());
    let health = restored
        .board
        .sources
        .iter()
        .find(|h| h.kind == Kind::Jira)
        .unwrap();
    assert_eq!(health.status, "error");
    assert!(health.last_success_at.is_none());
    // A Jira-only discovery change must not invalidate unrelated forge snapshots.
    assert_eq!(restored.board.pull_requests.len(), 1);
    assert!(
        !restored.board.pull_requests[0]
            .linked_issue_keys
            .contains(&"PRIVATE-99".into())
    );
}

#[tokio::test]
async fn account_change_purges_derived_cross_source_links_and_prior_success_before_failure() {
    use pullboard::model::Kind;
    let mut c = config();
    c.jira = Some(jira("https://jira.example"));
    c.forges = vec![forge("github", "https://forge.example")];
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    cache
        .publish(vec![
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![issue()],
                    issue_weblinks: vec![("PRIVATE-99".into(), pr().url)],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    assert!(
        cache.read().await.unwrap().board.pull_requests[0]
            .linked_issue_keys
            .contains(&"PRIVATE-99".into())
    );
    cache
        .verify(identity("jira", Kind::Jira, "another-account"))
        .await
        .unwrap();
    let s = cache.read().await.unwrap();
    assert!(s.board.issues.is_empty());
    assert!(s.issue_weblinks.is_empty());
    assert!(
        !s.board.pull_requests[0]
            .linked_issue_keys
            .contains(&"PRIVATE-99".into())
    );
    let h = s
        .board
        .sources
        .iter()
        .find(|h| h.source_id == "jira")
        .unwrap();
    assert!(h.last_success_at.is_none());
    assert_eq!(h.status, "pending");
}

#[tokio::test]
async fn concurrent_readers_see_old_or_new_combined_snapshot_never_half_publication() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("concurrent.sqlite");
    c.forges = vec![forge("github", "https://api.github.com")];
    c.jira = Some(jira("https://jira.example"));
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    verify_sources(&cache, &c).await;
    let writer = cache.clone();
    let task = tokio::spawn(async move {
        for revision in 1..=30 {
            let mut p = pr();
            p.title = revision.to_string();
            let mut i = issue();
            i.title = revision.to_string();
            writer
                .publish(vec![
                    SourceUpdate {
                        source_id: "test-forge".into(),
                        result: Ok(SourceData {
                            pull_requests: vec![p],
                            ..Default::default()
                        }),
                    },
                    SourceUpdate {
                        source_id: "jira".into(),
                        result: Ok(SourceData {
                            issues: vec![i],
                            ..Default::default()
                        }),
                    },
                ])
                .await
                .unwrap();
        }
    });
    for _ in 0..100 {
        let s = cache.read().await.unwrap();
        if s.board.generated_at.is_none() {
            assert!(s.board.issues.is_empty() && s.board.pull_requests.is_empty());
        } else {
            assert_eq!(s.board.issues.len(), 1);
            assert_eq!(s.board.pull_requests.len(), 1);
            assert_eq!(s.board.issues[0].title, s.board.pull_requests[0].title);
        }
    }
    task.await.unwrap();
}

#[tokio::test]
async fn tightening_configured_repository_and_project_filters_also_prunes_warm_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("filtered.sqlite");
    c.forges = vec![forge("github", "https://api.github.com")];
    c.jira = Some(jira("https://jira.example"));
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    cache
        .publish(vec![
            SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    ..Default::default()
                }),
            },
            SourceUpdate {
                source_id: "jira".into(),
                result: Ok(SourceData {
                    issues: vec![issue()],
                    ..Default::default()
                }),
            },
        ])
        .await
        .unwrap();
    drop(cache);
    c.forges[0].exclude_repositories = vec!["acme/*".into()];
    c.jira.as_mut().unwrap().projects.clear();
    let cache = Cache::open(&c.server.database, &c).await.unwrap();
    verify_sources(&cache, &c).await;
    let board = cache.read().await.unwrap().board;
    assert!(board.issues.is_empty());
    assert!(board.pull_requests.is_empty());
}
