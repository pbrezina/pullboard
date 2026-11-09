mod common;
use async_trait::async_trait;
use common::*;
use pullboard::{
    cache::{Cache, SourceData, SourceUpdate},
    error::{Error, Result},
    forge::Forge,
    model::*,
    sync::{Progress, Syncer},
};
use std::sync::Arc;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

struct Steps {
    id: String,
    steps: mpsc::UnboundedSender<SourcePhase>,
    release: Arc<Semaphore>,
    failure: Option<Error>,
}
#[async_trait]
impl Forge for Steps {
    fn source_id(&self) -> &str {
        &self.id
    }
    async fn identity(&self) -> Result<Identity> {
        Ok(identity(&self.id, Kind::Github, "1"))
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        progress.discovering();
        self.steps.send(SourcePhase::Discovering).unwrap();
        self.release.acquire().await.unwrap().forget();
        progress.fetching(Some(3));
        progress.item_completed();
        self.steps.send(SourcePhase::Fetching).unwrap();
        self.release.acquire().await.unwrap().forget();
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        progress.item_completed();
        progress.item_completed();
        let mut p = pr();
        p.source_id = self.id.clone();
        p.id = format!("{}:acme/widget:42", self.id);
        Ok(SourceData {
            pull_requests: vec![p],
            coverage_notes: vec!["Neutral mock coverage note".into()],
            ..Default::default()
        })
    }
}
fn steps(
    id: &str,
    failure: Option<Error>,
) -> (
    Arc<dyn Forge>,
    mpsc::UnboundedReceiver<SourcePhase>,
    Arc<Semaphore>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    (
        Arc::new(Steps {
            id: id.into(),
            steps: tx,
            release: release.clone(),
            failure,
        }),
        rx,
        release,
    )
}
async fn receive(rx: &mut mpsc::UnboundedReceiver<SourcePhase>, expected: SourcePhase) {
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .unwrap(),
        Some(expected)
    );
}
async fn idle(sync: &Syncer) -> SyncStatus {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status = sync.status().await.unwrap();
            if status.state == "idle" {
                validate_contract(&serde_json::to_value(&status).unwrap(), "SyncStatus");
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
async fn phase(sync: &Syncer, index: usize, expected: SourcePhase) -> SyncStatus {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status = sync.status().await.unwrap();
            if status.progress[index].phase == expected {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn progress_evolves_coherently_coalesces_counts_evaluated_items_and_waits_for_atomic_publication()
 {
    let mut c = config();
    let first = forge("github", "https://forge.example");
    let mut second = first.clone();
    second.id = "second".into();
    c.forges = vec![first, second];
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
    let (one, mut rx1, release1) = steps("test-forge", None);
    let (two, mut rx2, release2) = steps("second", None);
    let sync = Syncer::new(cache.clone(), vec![one, two], None, 3600);
    let initial = sync.status().await.unwrap();
    assert!(
        initial
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Idle)
    );
    validate_contract(&serde_json::to_value(&initial).unwrap(), "SyncStatus");
    let queued = sync.request().await.unwrap();
    assert!(
        queued
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Queued)
    );
    validate_contract(&serde_json::to_value(&queued).unwrap(), "SyncStatus");
    let cancel = CancellationToken::new();
    let task = sync.spawn(cancel.clone());
    receive(&mut rx1, SourcePhase::Discovering).await;
    receive(&mut rx2, SourcePhase::Discovering).await;
    let discovering = sync.status().await.unwrap();
    assert_eq!(discovering.progress[0].total_items, None);
    assert_eq!(discovering.progress[0].completed_items, 0);
    assert_eq!(discovering.progress[1].phase, SourcePhase::Discovering);
    validate_contract(&serde_json::to_value(&discovering).unwrap(), "SyncStatus");
    release1.add_permits(1);
    receive(&mut rx1, SourcePhase::Fetching).await;
    let fetching = sync.status().await.unwrap();
    assert_eq!(fetching.progress[0].completed_items, 1);
    assert_eq!(fetching.progress[0].total_items, Some(3));
    let coalesced = sync.request().await.unwrap();
    assert_eq!(coalesced.started_at, queued.started_at);
    assert_eq!(
        coalesced.progress[0].started_at,
        fetching.progress[0].started_at
    );
    assert_eq!(coalesced.progress[0].completed_items, 1);
    release1.add_permits(1);
    let publishing = phase(&sync, 0, SourcePhase::Publishing).await;
    assert_eq!(publishing.progress[0].phase, SourcePhase::Publishing);
    assert_eq!(publishing.progress[0].total_items, Some(3));
    assert!(publishing.sources[0].last_success_at.is_none());
    assert!(cache.read().await.unwrap().board.pull_requests.is_empty());
    let revision = cache.read().await.unwrap().board.revision;
    tokio::time::sleep(std::time::Duration::from_millis(12)).await;
    let waiting = sync.status().await.unwrap();
    assert!(waiting.progress[0].elapsed_ms > publishing.progress[0].elapsed_ms);
    assert_eq!(revision, cache.read().await.unwrap().board.revision);
    release2.add_permits(1);
    receive(&mut rx2, SourcePhase::Fetching).await;
    release2.add_permits(1);
    let completed = idle(&sync).await;
    assert!(completed.last_success_at.is_some());
    assert!(
        completed
            .sources
            .iter()
            .all(|s| s.status == "ok" && !s.coverage_notes.is_empty())
    );
    assert!(
        completed
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Completed
                && p.completed_items == 3
                && p.total_items == Some(3))
    );
    let board = cache.read().await.unwrap().board;
    assert_eq!(board.pull_requests.len(), 2);
    assert_eq!(board.revision, revision + 1);
    assert_eq!(
        completed
            .sources
            .iter()
            .map(|s| s.source_id.as_str())
            .collect::<Vec<_>>(),
        vec!["test-forge", "second"]
    );
    let duration = completed.progress[0].elapsed_ms;
    tokio::time::sleep(std::time::Duration::from_millis(12)).await;
    assert_eq!(
        sync.status().await.unwrap().progress[0].elapsed_ms,
        duration
    );
    // New acceptance resets every source, unlike coalescing.
    let reset = sync.request().await.unwrap();
    assert!(reset.finished_at.is_none());
    assert!(reset.progress.iter().all(|p| p.phase == SourcePhase::Queued
        && p.completed_items == 0
        && p.total_items.is_none()
        && p.started_at.is_none()
        && p.finished_at.is_none()
        && p.elapsed_ms == 0));
    cancel.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn failure_and_backoff_retain_old_cards_and_freeze_honest_attempt_counts_then_reset_skips() {
    for rate_limited in [false, true] {
        let mut c = config();
        c.forges = vec![forge("github", "https://forge.example")];
        let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
        verify_sources(&cache, &c).await;
        let old = cache
            .publish(vec![SourceUpdate {
                source_id: "test-forge".into(),
                result: Ok(SourceData {
                    pull_requests: vec![pr()],
                    coverage_notes: vec!["Old neutral note".into()],
                    ..Default::default()
                }),
            }])
            .await
            .unwrap();
        let error = if rate_limited {
            Error::backoff(chrono::Utc::now() + chrono::Duration::minutes(5))
        } else {
            Error::upstream()
        };
        let (adapter, mut rx, release) = steps("test-forge", Some(error));
        let sync = Syncer::new(cache.clone(), vec![adapter], None, 3600);
        let cancel = CancellationToken::new();
        let task = sync.spawn(cancel.clone());
        receive(&mut rx, SourcePhase::Discovering).await;
        release.add_permits(1);
        receive(&mut rx, SourcePhase::Fetching).await;
        release.add_permits(1);
        let failed = idle(&sync).await;
        assert_eq!(
            failed.progress[0].phase,
            if rate_limited {
                SourcePhase::Backoff
            } else {
                SourcePhase::Failed
            }
        );
        assert_eq!(failed.progress[0].completed_items, 1);
        assert_eq!(failed.progress[0].total_items, Some(3));
        assert!(failed.progress[0].finished_at.is_some());
        assert!(failed.last_success_at.is_none());
        assert_eq!(
            failed.sources[0].last_success_at,
            old.board.sources[0].last_success_at
        );
        assert_eq!(
            failed.sources[0].coverage_notes,
            old.board.sources[0].coverage_notes
        );
        let board = cache.read().await.unwrap().board;
        assert_eq!(board.pull_requests[0].title, pr().title);
        assert_eq!(
            failed.sources[0].last_attempt_at,
            failed.progress[0].started_at
        );
        validate_contract(&serde_json::to_value(&board).unwrap(), "Board");
        if rate_limited {
            let reset = sync.request().await.unwrap();
            let p = &reset.progress[0];
            assert_eq!(p.phase, SourcePhase::Backoff);
            assert_eq!(p.completed_items, 0);
            assert_eq!(p.total_items, None);
            assert_eq!(p.started_at, None);
            assert_eq!(p.finished_at, None);
            assert_eq!(p.elapsed_ms, 0);
            let skipped = idle(&sync).await;
            assert_eq!(
                skipped.sources[0].last_attempt_at,
                failed.sources[0].last_attempt_at
            );
            assert!(skipped.last_success_at.is_none());
            assert_eq!(skipped.sources[0].status, "backoff");
            assert!(rx.try_recv().is_err());
        }
        cancel.cancel();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn publication_failure_is_live_not_green_and_preserves_old_board_revision_and_success_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("publication.sqlite");
    let first = forge("github", "https://forge.example");
    let mut second = first.clone();
    second.id = "second".into();
    c.forges = vec![first, second];
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
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
    let (adapter, mut rx, release) = steps("test-forge", None);
    let (second, mut rx2, release2) = steps("second", None);
    let sync = Syncer::new(cache.clone(), vec![adapter, second], None, 3600);
    let cancel = CancellationToken::new();
    let task = sync.spawn(cancel.clone());
    receive(&mut rx, SourcePhase::Discovering).await;
    receive(&mut rx2, SourcePhase::Discovering).await;
    release.add_permits(1);
    receive(&mut rx, SourcePhase::Fetching).await;
    release.add_permits(1);
    phase(&sync, 0, SourcePhase::Publishing).await;
    let old = serde_json::to_value(cache.read().await.unwrap().board).unwrap();
    let db = sqlx::SqlitePool::connect(c.server.database.to_str().unwrap())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_publication BEFORE UPDATE ON pullboard_snapshot_v1 BEGIN SELECT RAISE(FAIL, 'mock cache failure'); END").execute(&db).await.unwrap();
    release2.add_permits(1);
    receive(&mut rx2, SourcePhase::Fetching).await;
    release2.add_permits(1);
    let failed = idle(&sync).await;
    assert_eq!(failed.sources[0].status, "error");
    assert_eq!(failed.progress[0].phase, SourcePhase::Failed);
    assert_eq!(failed.progress[0].completed_items, 3);
    assert!(failed.sources.iter().all(|h| h.status == "error"));
    assert!(
        failed
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Failed
                && p.completed_items == 3
                && p.total_items == Some(3))
    );
    assert_eq!(
        failed.sources[0].last_success_at,
        old["sources"][0]["last_success_at"]
            .as_str()
            .map(|s| s.parse().unwrap())
    );
    assert_eq!(
        old,
        serde_json::to_value(cache.read().await.unwrap().board).unwrap()
    );
    assert!(failed.last_success_at.is_none());
    cancel.cancel();
    task.await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn early_source_failure_is_visible_before_board_publication_and_backoff_survives_a_failed_commit()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config();
    c.server.database = dir.path().join("backoff.sqlite");
    let first = forge("github", "https://forge.example");
    let mut second = first.clone();
    second.id = "second".into();
    c.forges = vec![first, second];
    let cache = Arc::new(Cache::open(&c.server.database, &c).await.unwrap());
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
    let (one, mut rx1, release1) = steps(
        "test-forge",
        Some(Error::backoff(
            chrono::Utc::now() + chrono::Duration::minutes(5),
        )),
    );
    let (two, mut rx2, release2) = steps("second", None);
    let sync = Syncer::new(cache.clone(), vec![one, two], None, 3600);
    let cancel = CancellationToken::new();
    let task = sync.spawn(cancel.clone());
    receive(&mut rx1, SourcePhase::Discovering).await;
    receive(&mut rx2, SourcePhase::Discovering).await;
    release1.add_permits(1);
    receive(&mut rx1, SourcePhase::Fetching).await;
    release1.add_permits(1);
    let live = phase(&sync, 0, SourcePhase::Backoff).await;
    assert_eq!(live.sources[0].status, "backoff");
    assert_eq!(live.progress[0].phase, SourcePhase::Backoff);
    let duration = live.progress[0].elapsed_ms;
    assert_eq!(cache.read().await.unwrap().board.sources[0].status, "ok");
    assert_eq!(cache.read().await.unwrap().board.pull_requests.len(), 1);
    let db = sqlx::SqlitePool::connect(c.server.database.to_str().unwrap())
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_publication BEFORE UPDATE ON pullboard_snapshot_v1 BEGIN SELECT RAISE(FAIL, 'mock cache failure'); END").execute(&db).await.unwrap();
    release2.add_permits(1);
    receive(&mut rx2, SourcePhase::Fetching).await;
    release2.add_permits(1);
    let failed = idle(&sync).await;
    assert_eq!(failed.sources[0].status, "backoff");
    assert_eq!(failed.sources[1].status, "error");
    assert_eq!(failed.progress[0].elapsed_ms, duration);
    sqlx::query("DROP TRIGGER fail_publication")
        .execute(&db)
        .await
        .unwrap();
    let reset = sync.request().await.unwrap();
    assert_eq!(reset.progress[0].phase, SourcePhase::Backoff);
    assert_eq!(reset.progress[0].started_at, None);
    assert_eq!(reset.progress[0].completed_items, 0);
    receive(&mut rx2, SourcePhase::Discovering).await;
    release2.add_permits(1);
    receive(&mut rx2, SourcePhase::Fetching).await;
    release2.add_permits(1);
    let skipped = idle(&sync).await;
    assert_eq!(skipped.sources[0].status, "backoff");
    assert_eq!(
        skipped.sources[0].last_attempt_at,
        failed.sources[0].last_attempt_at
    );
    assert_eq!(
        cache.read().await.unwrap().board.sources[0].status,
        "backoff"
    );
    assert!(skipped.last_success_at.is_none());
    assert!(rx1.try_recv().is_err());
    cancel.cancel();
    task.await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn preparation_and_pre_attempt_infrastructure_failures_exit_active_phases_with_honest_zero_counters()
 {
    for before_attempt in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let server = wiremock::MockServer::start().await;
        let mut c = config();
        c.server.database = dir.path().join("early.sqlite");
        c.forges = vec![forge("forgejo", &server.uri())];
        let app = pullboard::api::App::build(c.clone()).await.unwrap();
        if before_attempt {
            let db = sqlx::SqlitePool::connect(c.server.database.to_str().unwrap())
                .await
                .unwrap();
            sqlx::query("DROP TABLE pullboard_snapshot_v1")
                .execute(&db)
                .await
                .unwrap();
            db.close().await;
        }
        let cancel = CancellationToken::new();
        let task = app.sync.spawn(cancel.clone());
        app.sync.request().await.unwrap();
        let failed = idle(&app.sync).await;
        let p = &failed.progress[0];
        assert_eq!(p.phase, SourcePhase::Failed);
        assert_eq!(p.completed_items, 0);
        assert_eq!(p.total_items, None);
        assert_eq!(p.started_at.is_none(), before_attempt);
        assert!(p.finished_at.is_some());
        if before_attempt {
            assert_eq!(p.elapsed_ms, 0);
        }
        assert_eq!(failed.sources[0].status, "error");
        assert_eq!(failed.sources[0].last_attempt_at, p.started_at);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            usize::from(!before_attempt)
        );
        assert!(failed.last_success_at.is_none());
        cancel.cancel();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn all_five_real_adapters_succeed_with_neutral_notes_and_empty_work_and_zero_sources_is_noop()
{
    let server = wiremock::MockServer::start().await;
    mock_jira(&server).await;
    mock_get(
        &server,
        "/rest/api/3/search/jql",
        serde_json::json!({"issues":[],"isLast":true}),
    )
    .await;
    mock_get(&server, "/user", github_user(1, "me")).await;
    mock_get(
        &server,
        "/search/issues",
        serde_json::json!({"total_count":0,"incomplete_results":false,"items":[]}),
    )
    .await;
    mock_get(&server, "/api/v4/user", gitlab_user(1, "me")).await;
    mock_get(&server, "/api/v4/merge_requests", serde_json::json!([])).await;
    mock_get(&server, "/api/v4/todos", serde_json::json!([])).await;
    mock_get(&server, "/api/v1/user", forgejo_user(1, "me")).await;
    mock_get(
        &server,
        "/api/v1/repos/issues/search",
        serde_json::json!([]),
    )
    .await;
    for empty in [false, true] {
        let mut c = config();
        if !empty {
            c.jira = Some(jira(&server.uri()));
            c.forges = ["github", "gitlab", "codeberg", "forgejo"]
                .into_iter()
                .map(|kind| {
                    let mut f = forge(kind, &server.uri());
                    f.id = kind.into();
                    f
                })
                .collect();
        }
        let app = pullboard::api::App::build(c).await.unwrap();
        let cancel = CancellationToken::new();
        let task = app.sync.spawn(cancel.clone());
        // Wait for enqueue as startup begins inside the worker task.
        app.sync.request().await.unwrap();
        let status = idle(&app.sync).await;
        assert!(status.last_success_at.is_some());
        assert_eq!(status.sources.len(), if empty { 0 } else { 5 });
        assert!(status.sources.iter().all(|h| h.status == "ok"
            && !h.coverage_notes.is_empty()
            && h.error.is_none()
            && h.retry_at.is_none()));
        assert!(
            status
                .progress
                .iter()
                .all(|p| p.phase == SourcePhase::Completed
                    && p.completed_items == 0
                    && p.total_items == Some(0))
        );
        assert_eq!(
            status
                .sources
                .iter()
                .map(|s| &s.source_id)
                .collect::<Vec<_>>(),
            status
                .progress
                .iter()
                .map(|p| &p.source_id)
                .collect::<Vec<_>>()
        );
        let board = app.cache.read().await.unwrap().board;
        validate_contract(&serde_json::to_value(&board).unwrap(), "Board");
        if empty {
            assert_eq!(board.revision, 0);
            assert_eq!(board.generated_at, None);
        }
        cancel.cancel();
        task.await.unwrap();
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}
