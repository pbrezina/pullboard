//! One coalescing worker. Live progress/health is independent of atomic card publication.
use crate::{
    cache::{Cache, SourceData, SourceUpdate},
    error::{Error, Result},
    forge::Forge,
    jira::Jira,
    model::*,
};
use chrono::Utc;
use futures_util::{StreamExt, stream};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

// Source reads borrow the adapters and stay inside the single refresh worker.
// Per-origin HTTP request budgets are enforced separately by the transports.
const MAX_CONCURRENT_SOURCES: usize = 4;

enum Source<'a> {
    Jira(&'a Jira),
    Forge(&'a dyn Forge),
}
impl Source<'_> {
    fn id(&self) -> &str {
        match self {
            Self::Jira(_) => "jira",
            Self::Forge(forge) => forge.source_id(),
        }
    }
    async fn identity(&self) -> Result<Identity> {
        match self {
            Self::Jira(jira) => jira.identity().await,
            Self::Forge(forge) => forge.identity().await,
        }
    }
    async fn discover(&self, progress: Progress) -> Result<SourceData> {
        match self {
            Self::Jira(jira) => jira.discover_with_progress(progress).await,
            Self::Forge(forge) => forge.discover_with_progress(progress).await,
        }
    }
}

struct Live {
    status: SyncStatus,
    clocks: Vec<Option<Instant>>,
    write_refresh_pending: bool,
    scopes: Vec<Option<String>>,
}
impl Live {
    fn sample(&self) -> SyncStatus {
        let mut status = self.status.clone();
        let now = Instant::now();
        for (p, clock) in status.progress.iter_mut().zip(&self.clocks) {
            if let Some(start) = clock {
                p.elapsed_ms = elapsed(*start, now);
            }
        }
        status
    }
    fn enqueue(&mut self) {
        self.status.state = "syncing".into();
        self.status.started_at = Some(Utc::now());
        self.status.finished_at = None;
        self.status.progress = self
            .status
            .sources
            .iter()
            .map(|s| {
                SourceProgress::new(
                    s.source_id.clone(),
                    if s.retry_at.is_some_and(|at| at > Utc::now()) {
                        SourcePhase::Backoff
                    } else {
                        SourcePhase::Queued
                    },
                )
            })
            .collect();
        self.clocks.fill(None);
    }
    fn finish_source(&mut self, index: usize, phase: SourcePhase) {
        let p = &mut self.status.progress[index];
        p.phase = phase;
        p.finished_at = Some(Utc::now());
        if let Some(start) = self.clocks[index].take() {
            p.elapsed_ms = elapsed(start, Instant::now());
        }
    }
}
fn elapsed(start: Instant, now: Instant) -> u64 {
    u64::try_from(now.duration_since(start).as_millis()).unwrap_or(u64::MAX)
}
fn fail_health(health: &mut SourceHealth, error: &Error) {
    health.status = if error.retry_at.is_some() {
        "backoff"
    } else {
        "error"
    }
    .into();
    health.error = Some(error.message.into());
    health.retry_at = error.retry_at;
}

/// Attempt-scoped adapter instrumentation. Updates share the sync status lock;
/// counters advance only after a deduplicated item has been fully evaluated.
#[derive(Clone, Default)]
pub struct Progress {
    live: Option<(Arc<Mutex<Live>>, usize)>,
}
impl Progress {
    /// Standalone recorder for adapter tests; production reporters belong to a job.
    pub fn recording(source_id: &str) -> Self {
        Self {
            live: Some((
                Arc::new(Mutex::new(Live {
                    status: SyncStatus {
                        progress: vec![SourceProgress {
                            started_at: Some(Utc::now()),
                            ..SourceProgress::new(source_id.into(), SourcePhase::Preparing)
                        }],
                        ..Default::default()
                    },
                    clocks: vec![Some(Instant::now())],
                    write_refresh_pending: false,
                    scopes: vec![None],
                })),
                0,
            )),
        }
    }
    pub fn snapshot(&self) -> Option<SourceProgress> {
        self.live.as_ref().map(|(live, index)| {
            live.lock().expect("sync lock poisoned").sample().progress[*index].clone()
        })
    }
    fn update(&self, f: impl FnOnce(&mut SourceProgress)) {
        if let Some((live, index)) = &self.live {
            f(&mut live.lock().expect("sync lock poisoned").status.progress[*index]);
        }
    }
    pub fn discovering(&self) {
        self.update(|p| {
            p.phase = SourcePhase::Discovering;
            p.total_items = None;
        });
    }
    pub fn fetching(&self, total: Option<u64>) {
        self.update(|p| {
            p.phase = SourcePhase::Fetching;
            p.total_items = total.filter(|total| *total >= p.completed_items);
        });
    }
    pub fn item_completed(&self) {
        self.update(|p| {
            p.completed_items = p.completed_items.saturating_add(1);
            // An extending/interleaved inventory must not claim a false denominator.
            if p.total_items.is_some_and(|total| total < p.completed_items) {
                p.total_items = None;
            }
        });
    }
}

pub struct Syncer {
    cache: Arc<Cache>,
    forges: Vec<Arc<dyn Forge>>,
    jira: Option<Arc<Jira>>,
    live: Arc<Mutex<Live>>,
    notify: Notify,
    interval: u64,
}
impl Syncer {
    pub fn new(
        cache: Arc<Cache>,
        forges: Vec<Arc<dyn Forge>>,
        jira: Option<Arc<Jira>>,
        interval: u64,
    ) -> Arc<Self> {
        if let Some(jira) = &jira {
            jira.bind_cache(cache.clone());
        }
        let sources = cache.configured_sources();
        let progress = sources
            .iter()
            .map(|s| SourceProgress::new(s.source_id.clone(), SourcePhase::Idle))
            .collect();
        let clocks = vec![None; sources.len()];
        let scopes = vec![None; sources.len()];
        Arc::new(Self {
            cache,
            forges,
            jira,
            live: Arc::new(Mutex::new(Live {
                status: SyncStatus {
                    sources,
                    progress,
                    ..Default::default()
                },
                clocks,
                write_refresh_pending: false,
                scopes,
            })),
            notify: Notify::new(),
            interval,
        })
    }
    pub async fn status(&self) -> Result<SyncStatus> {
        Ok(self.live.lock().expect("sync lock poisoned").sample())
    }
    pub async fn request(&self) -> Result<SyncStatus> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        if live.status.state != "syncing" {
            live.enqueue();
            self.notify.notify_one();
        }
        Ok(live.sample())
    }
    /// Unlike manual coalescing, a successful live write requires one follow-up
    /// if it raced with an already running read. Simulations never call this.
    pub async fn after_write(&self) -> Result<SyncStatus> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        if live.status.state == "syncing" {
            live.write_refresh_pending = true;
        } else {
            live.enqueue();
            self.notify.notify_one();
        }
        Ok(live.sample())
    }
    pub fn spawn(self: &Arc<Self>, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = this.request().await {
                tracing::error!(code = error.code, "could not schedule initial refresh");
            }
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(this.interval));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                tokio::select! {
                    _=cancel.cancelled()=>break,
                    _=interval.tick()=>{if let Err(error)=this.request().await{tracing::error!(code=error.code,"could not schedule refresh");}},
                    _=this.notify.notified()=>{
                        tokio::select!{_ = cancel.cancelled()=>break,_ = this.refresh()=>{}}
                    }
                }
            }
        })
    }
    fn start(&self, id: &str) -> Option<(usize, Progress)> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        let index = live.status.sources.iter().position(|s| s.source_id == id)?;
        if live.status.progress[index].phase == SourcePhase::Backoff {
            return None;
        }
        let now = Utc::now();
        live.status.sources[index].last_attempt_at = Some(now);
        live.status.progress[index].started_at = Some(now);
        live.status.progress[index].phase = SourcePhase::Preparing;
        live.clocks[index] = Some(Instant::now());
        Some((
            index,
            Progress {
                live: Some((self.live.clone(), index)),
            },
        ))
    }
    async fn verify(&self, index: usize, identity: Identity) -> Result<()> {
        let (source_id, kind) = {
            let live = self.live.lock().expect("sync lock poisoned");
            let source = &live.status.sources[index];
            (source.source_id.clone(), source.kind)
        };
        if identity.source_id != source_id || identity.kind != kind {
            return Err(Error::upstream());
        }
        self.cache.verify(identity).await?;
        let snapshot = self.cache.read().await?;
        // Another source may have verified between these awaits. Reconcile only
        // this source's principal scope and health, never a whole shared snapshot.
        let scope = snapshot.source_accounts.get(&source_id).cloned();
        let health = snapshot
            .board
            .sources
            .iter()
            .find(|s| s.source_id == source_id && s.kind == kind)
            .ok_or_else(Error::internal)?;
        let mut live = self.live.lock().expect("sync lock poisoned");
        // Account quarantine/restoration overrides old live health as well as cards.
        if live.scopes[index] != scope {
            let attempt = live.status.sources[index].last_attempt_at;
            live.status.sources[index] = health.clone();
            live.status.sources[index].last_attempt_at = attempt;
            live.scopes[index] = scope;
        }
        Ok(())
    }
    fn fetched(&self, index: usize, result: &Result<SourceData>) {
        let mut live = self.live.lock().expect("sync lock poisoned");
        match result {
            Ok(_) => {
                let p = &mut live.status.progress[index];
                p.phase = SourcePhase::Publishing;
                p.total_items = Some(p.completed_items);
            }
            Err(error) => {
                fail_health(&mut live.status.sources[index], error);
                live.finish_source(
                    index,
                    if error.retry_at.is_some() {
                        SourcePhase::Backoff
                    } else {
                        SourcePhase::Failed
                    },
                );
            }
        }
        let duration_ms = live.clocks[index]
            .map(|start| elapsed(start, Instant::now()))
            .unwrap_or(live.status.progress[index].elapsed_ms);
        tracing::info!(
            source_id = %live.status.sources[index].source_id,
            completed_items = live.status.progress[index].completed_items,
            elapsed_ms = duration_ms,
            succeeded = result.is_ok(),
            "source refresh reads finished"
        );
    }
    fn finish(&self, published: Result<crate::cache::Snapshot>, success: bool) {
        let mut live = self.live.lock().expect("sync lock poisoned");
        let committed = published.is_ok();
        match published {
            Ok(snapshot) => {
                live.status.sources = snapshot.board.sources;
                for index in 0..live.status.progress.len() {
                    if live.status.progress[index].phase == SourcePhase::Publishing {
                        let phase = if live.status.sources[index].status == "ok" {
                            SourcePhase::Completed
                        } else {
                            SourcePhase::Failed
                        };
                        live.finish_source(index, phase);
                    }
                }
            }
            Err(error) => {
                tracing::error!(code = error.code, "snapshot publication failed");
                for index in 0..live.status.progress.len() {
                    if matches!(
                        live.status.progress[index].phase,
                        SourcePhase::Publishing
                            | SourcePhase::Queued
                            | SourcePhase::Preparing
                            | SourcePhase::Discovering
                            | SourcePhase::Fetching
                    ) {
                        fail_health(&mut live.status.sources[index], &error);
                        live.finish_source(index, SourcePhase::Failed);
                    }
                }
            }
        }
        live.status.state = "idle".into();
        live.status.finished_at = Some(Utc::now());
        if committed
            && success
            && live
                .status
                .progress
                .iter()
                .all(|p| p.phase == SourcePhase::Completed)
        {
            live.status.last_success_at = live.status.finished_at;
        }
        if live.write_refresh_pending {
            live.write_refresh_pending = false;
            live.enqueue();
            self.notify.notify_one();
        }
    }
    async fn read_source(&self, source: Source<'_>) -> Option<(usize, SourceUpdate)> {
        let (index, progress) = self.start(source.id())?;
        let result = match source.identity().await {
            Ok(identity) => match self.verify(index, identity).await {
                Ok(()) => source.discover(progress).await,
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        self.fetched(index, &result);
        Some((
            index,
            SourceUpdate {
                source_id: source.id().into(),
                result,
            },
        ))
    }
    async fn refresh(&self) {
        if self
            .live
            .lock()
            .expect("sync lock poisoned")
            .status
            .sources
            .is_empty()
        {
            self.finish(Ok(crate::cache::Snapshot::default()), true);
            return;
        }
        let old = match self.cache.read().await {
            Ok(s) => s,
            Err(e) => {
                self.finish(Err(e), false);
                return;
            }
        };
        let mut reads = Vec::with_capacity(old.board.sources.len());
        if let Some(jira) = &self.jira {
            reads.push(self.read_source(Source::Jira(jira.as_ref())));
        }
        for forge in &self.forges {
            reads.push(self.read_source(Source::Forge(forge.as_ref())));
        }
        let mut reads = stream::iter(reads).buffer_unordered(MAX_CONCURRENT_SOURCES);
        let mut outcomes = Vec::with_capacity(old.board.sources.len());
        // Do not short-circuit on failure: every independent source gets its turn.
        while let Some(outcome) = reads.next().await {
            if let Some(outcome) = outcome {
                outcomes.push(outcome);
            }
        }
        outcomes.sort_by_key(|(index, _)| *index);
        let updates: Vec<_> = outcomes.into_iter().map(|(_, update)| update).collect();
        let health = self
            .live
            .lock()
            .expect("sync lock poisoned")
            .status
            .sources
            .clone();
        let success =
            updates.len() == old.board.sources.len() && updates.iter().all(|u| u.result.is_ok());
        let published = self.cache.publish_attempts(updates, &health).await;
        if let Err(error) = &published
            && let Some(jira) = &self.jira
        {
            jira.choice_failure(error.clone()).await;
        }
        self.finish(published, success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn elapsed_uses_only_monotonic_clock_and_unknown_inventory_never_fabricates_totals() {
        let progress = Progress::recording("mock");
        let (live, _) = progress.live.as_ref().unwrap();
        {
            let mut live = live.lock().unwrap();
            live.status.progress[0].started_at = Some(Utc::now() + chrono::Duration::days(1));
            live.clocks[0] = Some(Instant::now() - std::time::Duration::from_millis(20));
        }
        progress.discovering();
        progress.fetching(None);
        progress.item_completed();
        let unknown = progress.snapshot().unwrap();
        assert_eq!(unknown.completed_items, 1);
        assert_eq!(unknown.total_items, None);
        assert!(unknown.elapsed_ms >= 20);
        progress.fetching(Some(1));
        progress.discovering(); // Invalidate a formerly complete inventory first.
        progress.fetching(None);
        progress.item_completed();
        progress.fetching(Some(1)); // Never publish a denominator below completed.
        assert_eq!(progress.snapshot().unwrap().total_items, None);
        progress.fetching(Some(2));
        assert_eq!(progress.snapshot().unwrap().total_items, Some(2));
    }
    #[test]
    fn retry_deadline_expiry_does_not_change_terminal_health_or_restart_progress() {
        let progress = Progress::recording("mock");
        let (live, _) = progress.live.as_ref().unwrap();
        let mut live = live.lock().unwrap();
        live.status.sources = vec![SourceHealth {
            source_id: "mock".into(),
            name: "Mock".into(),
            kind: Kind::Forgejo,
            status: "backoff".into(),
            last_success_at: None,
            last_attempt_at: Some(Utc::now()),
            error: Some("Mock rate limit".into()),
            retry_at: Some(Utc::now() - chrono::Duration::seconds(1)),
            coverage_notes: vec![],
        }];
        live.finish_source(0, SourcePhase::Backoff);
        let before = live.sample();
        let after = live.sample();
        assert_eq!(after.sources[0].status, "backoff");
        assert_eq!(after.progress[0].phase, SourcePhase::Backoff);
        assert_eq!(after.progress[0].elapsed_ms, before.progress[0].elapsed_ms);
        live.enqueue();
        assert_eq!(live.sample().progress[0].phase, SourcePhase::Queued);
        assert_eq!(live.sample().sources[0].status, "backoff");
    }
}
