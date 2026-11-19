//! One coalescing worker. Live progress/health is independent of atomic card publication.
use crate::{
    cache::{Cache, SourceData, SourceUpdate},
    error::{Error, Result},
    forge::Forge,
    jira::Jira,
    model::*,
};
use chrono::Utc;
use futures_util::{FutureExt, StreamExt, TryFutureExt, stream};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

// Source reads borrow the adapters and stay inside the single refresh worker.
// Per-origin HTTP request budgets are enforced separately by the transports.
const MAX_CONCURRENT_SOURCES: usize = 4;

// Created for every configured source before any future is polled, including
// queued sources. A guard lives through its card transaction, not options work.
struct CardAttempt(watch::Sender<usize>);
impl Drop for CardAttempt {
    fn drop(&mut self) {
        self.0.send_modify(|remaining| *remaining -= 1);
    }
}

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
}

struct Live {
    status: SyncStatus,
    clocks: Vec<Option<Instant>>,
    write_refresh_pending: bool,
    scopes: Vec<Option<String>>,
    options_clock: Option<Instant>,
    job_clock: Option<Instant>,
    first_card_published: bool,
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
        if let (Some(options), Some(start)) = (&mut status.jira_options, self.options_clock) {
            options.progress.elapsed_ms = elapsed(start, now);
        }
        status
    }
    fn reconcile(&mut self, committed: crate::cache::Committed) {
        self.status.board_revision = committed.revision;
        for index in 0..self.status.sources.len() {
            let id = self.status.sources[index].source_id.clone();
            let scope = committed.scopes.get(&id).cloned();
            let changed = scope != self.scopes[index];
            if let Some(health) = committed.sources.iter().find(|h| h.source_id == id)
                && (changed
                    || health.last_attempt_at > self.status.sources[index].last_attempt_at
                    || (health.last_attempt_at == self.status.sources[index].last_attempt_at
                        && health.last_success_at > self.status.sources[index].last_success_at))
            {
                let attempt = self.status.sources[index].last_attempt_at;
                self.status.sources[index] = health.clone();
                if self.clocks[index].is_some() {
                    self.status.sources[index].last_attempt_at = attempt;
                }
            }
            if changed && self.status.progress[index].phase == SourcePhase::Completed {
                fail_health(&mut self.status.sources[index], &Error::unavailable());
                self.finish_source(index, SourcePhase::Failed);
            }
            if id == "jira"
                && let Some(options) = &mut self.status.jira_options
            {
                if let Some(health) = &committed.options
                    && (changed
                        || health.last_attempt_at > options.health.last_attempt_at
                        || (health.last_attempt_at == options.health.last_attempt_at
                            && health.last_success_at > options.health.last_success_at))
                {
                    let attempt = options.health.last_attempt_at;
                    options.health = health.clone();
                    if self.options_clock.is_some() {
                        options.health.last_attempt_at = attempt;
                    }
                }
                options.ready = committed.options_ready;
                if changed && options.progress.phase == SourcePhase::Completed {
                    fail_health(&mut options.health, &Error::unavailable());
                    options.progress.phase = SourcePhase::Failed;
                    options.progress.finished_at = Some(Utc::now());
                }
            }
            self.scopes[index] = scope;
        }
    }
    fn enqueue(&mut self) {
        self.job_clock = Some(Instant::now());
        self.first_card_published = false;
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
        self.options_clock = None;
        if let Some(options) = &mut self.status.jira_options {
            options.progress = JiraOptionsProgress::new(
                if options.health.retry_at.is_some_and(|at| at > Utc::now()) {
                    SourcePhase::Backoff
                } else {
                    SourcePhase::Queued
                },
            );
        }
    }
    fn finish_source(&mut self, index: usize, phase: SourcePhase) {
        let p = &mut self.status.progress[index];
        p.phase = phase;
        p.finished_at = Some(Utc::now());
        if let Some(start) = self.clocks[index].take() {
            p.elapsed_ms = elapsed(start, Instant::now());
        }
    }
    fn finish_options(&mut self, phase: SourcePhase) {
        let Some(options) = &mut self.status.jira_options else {
            return;
        };
        options.progress.phase = phase;
        options.progress.finished_at = Some(Utc::now());
        if let Some(start) = self.options_clock.take() {
            options.progress.elapsed_ms = elapsed(start, Instant::now());
        }
        for row in options
            .progress
            .groups
            .iter_mut()
            .chain(&mut options.progress.projects)
        {
            if row.phase != SourcePhase::Completed {
                row.phase = phase;
            }
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
                    options_clock: None,
                    job_clock: None,
                    first_card_published: false,
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

/// Process-local creation-choice instrumentation. No account IDs enter status.
#[derive(Clone, Default)]
pub(crate) struct ChoiceReporter {
    live: Option<Arc<Mutex<Live>>>,
    discovery_complete: Arc<AtomicBool>,
}
impl ChoiceReporter {
    fn update(&self, f: impl FnOnce(&mut JiraOptionsProgress)) {
        if let Some(live) = &self.live
            && let Some(options) = &mut live.lock().expect("sync lock poisoned").status.jira_options
        {
            f(&mut options.progress);
        }
    }
    pub(crate) fn discovery(&self, groups: &[String], projects: &[String]) {
        self.update(|p| {
            p.phase = SourcePhase::Discovering;
            let row = |name: &String| ChoiceProgress {
                name: name.clone(),
                phase: SourcePhase::Queued,
                completed_items: 0,
                total_items: None,
            };
            p.groups = groups.iter().map(row).collect();
            p.projects = projects.iter().map(row).collect();
        });
    }
    pub(crate) fn group_started(&self, name: &str) {
        self.update(|p| {
            if let Some(row) = p.groups.iter_mut().find(|r| r.name == name) {
                row.phase = SourcePhase::Discovering;
            }
        });
    }
    pub(crate) fn group_completed(&self, name: &str, total: u64) {
        self.update(|p| {
            if let Some(row) = p.groups.iter_mut().find(|r| r.name == name) {
                row.phase = SourcePhase::Completed;
                row.total_items = Some(total);
                row.completed_items = total;
            }
        });
    }
    pub(crate) fn eligibility(&self, total: u64) {
        self.update(|p| {
            for row in &mut p.projects {
                row.total_items = Some(total);
                if total == 0 {
                    row.phase = SourcePhase::Completed;
                }
            }
            self.refresh_phase(p);
        });
    }
    fn refresh_phase(&self, p: &mut JiraOptionsProgress) {
        if self.discovery_complete.load(Ordering::Relaxed)
            && p.projects.iter().all(|r| r.total_items.is_some())
            && p.projects.iter().any(|r| r.phase != SourcePhase::Completed)
        {
            p.phase = SourcePhase::Fetching;
        } else {
            p.phase = SourcePhase::Discovering;
        }
    }
    pub(crate) fn metadata_completed(&self) {
        self.update(|p| {
            // Store and phase decisions share the live lock with eligibility.
            self.discovery_complete.store(true, Ordering::Relaxed);
            self.refresh_phase(p);
        });
    }
    pub(crate) fn project_started(&self, key: &str) {
        self.update(|p| {
            if let Some(row) = p.projects.iter_mut().find(|r| r.name == key) {
                row.phase = SourcePhase::Fetching;
            }
        });
    }
    pub(crate) fn project_checked(&self, key: &str) {
        self.update(|p| {
            if let Some(row) = p.projects.iter_mut().find(|r| r.name == key) {
                row.completed_items = row.completed_items.saturating_add(1);
                if row.total_items == Some(row.completed_items) {
                    row.phase = SourcePhase::Completed;
                }
            }
            self.refresh_phase(p);
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
        let jira_options = cache
            .configured_options_health()
            .map(|health| JiraOptionsStatus {
                ready: false,
                health,
                progress: JiraOptionsProgress::new(SourcePhase::Idle),
            });
        Arc::new(Self {
            cache,
            forges,
            jira,
            live: Arc::new(Mutex::new(Live {
                status: SyncStatus {
                    jira_options,
                    sources,
                    progress,
                    ..Default::default()
                },
                clocks,
                write_refresh_pending: false,
                scopes,
                options_clock: None,
                job_clock: None,
                first_card_published: false,
            })),
            notify: Notify::new(),
            interval,
        })
    }
    pub async fn status(&self) -> Result<SyncStatus> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(self.cache.committed());
        Ok(live.sample())
    }
    pub async fn request(&self) -> Result<SyncStatus> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(self.cache.committed());
        if live.status.state != "syncing" {
            live.enqueue();
            self.notify.notify_one();
        }
        Ok(live.sample())
    }
    /// Unlike manual coalescing, live ticket creation needs a follow-up if it
    /// races with an already running read. Standalone links publish directly.
    pub async fn after_write(&self) -> Result<SyncStatus> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(self.cache.committed());
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
                    _=cancel.cancelled()=>{ this.cancelled().await; break; },
                    _=interval.tick()=>{if let Err(error)=this.request().await{tracing::error!(code=error.code,"could not schedule refresh");}},
                    _=this.notify.notified()=>{
                        tokio::select!{_ = cancel.cancelled()=>{ this.cancelled().await; break; },_ = this.refresh()=>{}}
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
    async fn verify(&self, index: usize, identity: Identity) -> Result<String> {
        let (source_id, kind) = {
            let live = self.live.lock().expect("sync lock poisoned");
            let source = &live.status.sources[index];
            (source.source_id.clone(), source.kind)
        };
        if identity.source_id != source_id || identity.kind != kind {
            return Err(Error::upstream());
        }
        self.cache.verify(identity).await?;
        let committed = self.cache.committed();
        let scope = committed
            .scopes
            .get(&source_id)
            .cloned()
            .ok_or_else(Error::unavailable)?;
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(committed);
        Ok(scope)
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
    fn finish(&self) {
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(self.cache.committed());
        live.status.state = "idle".into();
        live.status.finished_at = Some(Utc::now());
        if live
            .status
            .progress
            .iter()
            .all(|p| p.phase == SourcePhase::Completed)
            && live
                .status
                .jira_options
                .as_ref()
                .is_none_or(|o| o.progress.phase == SourcePhase::Completed)
        {
            live.status.last_success_at = live.status.finished_at;
        }
        if live.write_refresh_pending {
            live.write_refresh_pending = false;
            live.enqueue();
            self.notify.notify_one();
        }
    }
    async fn publish_source(&self, index: usize, scope: Option<&str>, result: Result<SourceData>) {
        self.fetched(index, &result);
        let health = self.live.lock().expect("sync lock poisoned").status.sources[index].clone();
        let published = self
            .cache
            .publish_source(
                SourceUpdate {
                    source_id: health.source_id.clone(),
                    result,
                },
                health,
                scope,
            )
            .await;
        let mut live = self.live.lock().expect("sync lock poisoned");
        match published {
            Ok(snapshot) => {
                live.status.sources[index] = snapshot.board.sources[index].clone();
                if live.status.progress[index].phase == SourcePhase::Publishing {
                    live.finish_source(index, SourcePhase::Completed);
                }
                live.reconcile(self.cache.committed());
                tracing::info!(source_id = %live.status.sources[index].source_id, board_revision = snapshot.board.revision, elapsed_ms = live.status.progress[index].elapsed_ms, "source snapshot committed");
                if !live.first_card_published
                    && live.status.progress[index].phase == SourcePhase::Completed
                {
                    live.first_card_published = true;
                    tracing::info!(
                        elapsed_ms = live.job_clock.map(|start| elapsed(start, Instant::now())),
                        "first source card snapshot committed"
                    );
                }
            }
            Err(error) => {
                live.reconcile(self.cache.committed());
                // Preserve a transport backoff if persisting its health failed.
                if live.status.progress[index].phase != SourcePhase::Backoff {
                    fail_health(&mut live.status.sources[index], &error);
                    live.finish_source(index, SourcePhase::Failed);
                }
                tracing::error!(source_id = %live.status.sources[index].source_id, code = error.code, "source snapshot publication failed");
            }
        }
    }
    fn start_options(&self) -> Option<ChoiceReporter> {
        let mut live = self.live.lock().expect("sync lock poisoned");
        live.reconcile(self.cache.committed());
        let options = live.status.jira_options.as_mut()?;
        if options.progress.phase == SourcePhase::Backoff
            || options.health.retry_at.is_some_and(|at| at > Utc::now())
        {
            options.progress = JiraOptionsProgress::new(SourcePhase::Backoff);
            return None;
        }
        let now = Utc::now();
        options.health.last_attempt_at = Some(now);
        options.progress.started_at = Some(now);
        options.progress.phase = SourcePhase::Preparing;
        live.options_clock = Some(Instant::now());
        Some(ChoiceReporter {
            live: Some(self.live.clone()),
            ..Default::default()
        })
    }
    async fn publish_options(&self, scope: Option<String>, result: Result<crate::jira::Metadata>) {
        let mut health = {
            let mut live = self.live.lock().expect("sync lock poisoned");
            let options = live.status.jira_options.as_mut().expect("Jira configured");
            if let Err(error) = &result {
                fail_health(&mut options.health, error);
            } else {
                options.progress.phase = SourcePhase::Publishing;
            }
            options.health.clone()
        };
        let failure = result.as_ref().err().cloned();
        let published = self
            .cache
            .publish_options(scope.as_deref(), result, health.clone())
            .await;
        if let Some(error) = published.as_ref().err().or(failure.as_ref())
            && self.cache.committed().scopes.get("jira") == scope.as_ref()
            && let Some(jira) = &self.jira
        {
            jira.choice_failure(error.clone()).await;
        }
        let mut live = self.live.lock().expect("sync lock poisoned");
        match published {
            Ok(snapshot) => {
                let options = live.status.jira_options.as_mut().expect("Jira configured");
                options.health = snapshot.jira_options_health.expect("Jira configured");
                options.ready = snapshot.jira_choices.is_some();
                live.finish_options(failure.as_ref().map_or(SourcePhase::Completed, |e| {
                    if e.retry_at.is_some() {
                        SourcePhase::Backoff
                    } else {
                        SourcePhase::Failed
                    }
                }));
                live.reconcile(self.cache.committed());
                tracing::info!(
                    board_revision = snapshot.board.revision,
                    elapsed_ms = live
                        .status
                        .jira_options
                        .as_ref()
                        .map(|o| o.progress.elapsed_ms),
                    job_elapsed_ms = live.job_clock.map(|start| elapsed(start, Instant::now())),
                    ready = live.status.jira_options.as_ref().is_some_and(|o| o.ready),
                    succeeded = failure.is_none(),
                    "Jira options domain committed"
                );
            }
            Err(error) => {
                live.reconcile(self.cache.committed());
                if self.cache.committed().scopes.get("jira") != scope.as_ref() {
                    health = live
                        .status
                        .jira_options
                        .as_ref()
                        .expect("Jira configured")
                        .health
                        .clone();
                }
                if !failure.as_ref().is_some_and(|e| e.retry_at.is_some())
                    || self.cache.committed().scopes.get("jira") != scope.as_ref()
                {
                    fail_health(&mut health, &error);
                }
                let phase = if health.retry_at.is_some() {
                    SourcePhase::Backoff
                } else {
                    SourcePhase::Failed
                };
                live.status
                    .jira_options
                    .as_mut()
                    .expect("Jira configured")
                    .health = health;
                live.finish_options(phase);
                tracing::error!(code = error.code, "Jira options publication failed");
            }
        }
    }
    async fn read_source(
        &self,
        source: Source<'_>,
        card: CardAttempt,
        cards: watch::Receiver<usize>,
    ) {
        let initial_scope = self.cache.committed().scopes.get(source.id()).cloned();
        let Some((index, progress)) = self.start(source.id()) else {
            drop(card);
            if matches!(source, Source::Jira(_)) && self.start_options().is_some() {
                let error = self
                    .live
                    .lock()
                    .expect("sync lock poisoned")
                    .status
                    .sources
                    .iter()
                    .find(|s| s.source_id == source.id())
                    .and_then(|s| s.retry_at)
                    .filter(|at| *at > Utc::now())
                    .map_or_else(Error::unavailable, Error::backoff);
                self.publish_options(initial_scope, Err(error)).await;
            }
            return;
        };
        let verified = match source.identity().await {
            Ok(identity) => self
                .verify(index, identity.clone())
                .await
                .map(|scope| (identity, scope)),
            Err(error) => Err(error),
        };
        match source {
            Source::Forge(forge) => {
                let (scope, result) = match verified {
                    Ok((identity, scope)) => {
                        let mut result = match self.retry_error(index) {
                            Some(error) => Err(error),
                            None => forge.discover_verified(identity.clone(), progress).await,
                        };
                        if let Ok(data) = &mut result {
                            data.identity = Some(identity);
                        }
                        (Some(scope), result)
                    }
                    Err(error) => (initial_scope, Err(error)),
                };
                self.publish_source(index, scope.as_deref(), result).await;
                drop(card);
            }
            Source::Jira(jira) => {
                let (identity, scope) = match verified {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.publish_source(index, initial_scope.as_deref(), Err(error.clone()))
                            .await;
                        drop(card);
                        if self.start_options().is_some() {
                            self.publish_options(initial_scope, Err(error)).await;
                        }
                        return;
                    }
                };
                if let Some(error) = self.retry_error(index) {
                    self.publish_source(index, Some(&scope), Err(error.clone()))
                        .await;
                    drop(card);
                    if self.start_options().is_some() {
                        self.publish_options(Some(scope), Err(error)).await;
                    }
                    return;
                }
                let preparation = jira.prepare_tickets().map_ok(Arc::new).boxed().shared();
                let sprints = jira.sprints().map_ok(Arc::new).boxed().shared();
                let tickets = async {
                    let result = match preparation.clone().await {
                        Ok(preparation) => {
                            jira.read_tickets(
                                identity.clone(),
                                &preparation,
                                sprints.clone(),
                                progress,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    self.publish_source(index, Some(&scope), result).await;
                    drop(card);
                };
                let options = async {
                    if let Some(reporter) = self.start_options() {
                        let result = jira
                            .discover_choices(
                                &identity,
                                preparation.clone(),
                                sprints.clone(),
                                reporter,
                                cards,
                            )
                            .await;
                        self.publish_options(Some(scope.clone()), result).await;
                    }
                };
                // Branch failures never cancel the other domain, including its
                // shared prerequisite read or in-memory SQLite publication.
                tokio::join!(tickets, options);
            }
        }
    }
    fn retry_error(&self, index: usize) -> Option<Error> {
        let live = self.live.lock().expect("sync lock poisoned");
        let source = &live.status.sources[index];
        let options_retry = if source.source_id == "jira" {
            live.status
                .jira_options
                .as_ref()
                .and_then(|o| o.health.retry_at)
        } else {
            None
        };
        source
            .retry_at
            .max(options_retry)
            .filter(|at| *at > Utc::now())
            .map(Error::backoff)
    }
    async fn refresh(&self) {
        let started = Instant::now();
        if let Err(error) = self.cache.read().await {
            self.fail_active(&error);
            self.finish();
            return;
        }
        let count = self.forges.len() + usize::from(self.jira.is_some());
        let (remaining, cards) = watch::channel(count);
        let mut reads = Vec::new();
        if let Some(jira) = &self.jira {
            reads.push(self.read_source(
                Source::Jira(jira.as_ref()),
                CardAttempt(remaining.clone()),
                cards.clone(),
            ));
        }
        for forge in &self.forges {
            reads.push(self.read_source(
                Source::Forge(forge.as_ref()),
                CardAttempt(remaining.clone()),
                cards.clone(),
            ));
        }
        drop(remaining);
        let mut reads = stream::iter(reads).buffer_unordered(MAX_CONCURRENT_SOURCES);
        // Publications happen inside each owned future while other sources continue.
        while reads.next().await.is_some() {}
        self.finish();
        tracing::info!(
            elapsed_ms = elapsed(started, Instant::now()),
            "refresh job finished"
        );
    }
    fn fail_active(&self, error: &Error) {
        let mut live = self.live.lock().expect("sync lock poisoned");
        for index in 0..live.status.progress.len() {
            if !matches!(
                live.status.progress[index].phase,
                SourcePhase::Completed | SourcePhase::Failed | SourcePhase::Backoff
            ) {
                fail_health(&mut live.status.sources[index], error);
                live.finish_source(index, SourcePhase::Failed);
            }
        }
        if let Some(options) = &mut live.status.jira_options
            && !matches!(
                options.progress.phase,
                SourcePhase::Completed | SourcePhase::Failed | SourcePhase::Backoff
            )
        {
            fail_health(&mut options.health, error);
            live.finish_options(SourcePhase::Failed);
        }
    }
    async fn cancelled(&self) {
        self.fail_active(&Error::new(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "refresh_cancelled",
            "Refresh cancelled; complete cached data was retained.",
        ));
        let mut live = self.live.lock().expect("sync lock poisoned");
        if live.status.state == "syncing" {
            live.status.state = "idle".into();
            live.status.finished_at = Some(Utc::now());
        }
        live.write_refresh_pending = false;
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
    #[test]
    fn options_duration_is_monotonic_and_terminal_rows_freeze_without_reusing_prior_work() {
        let progress = Progress::recording("jira");
        let (shared, _) = progress.live.as_ref().unwrap();
        let mut live = shared.lock().unwrap();
        live.status.jira_options = Some(JiraOptionsStatus {
            ready: false,
            health: SourceHealth {
                source_id: "jira".into(),
                name: "Jira creation options".into(),
                kind: Kind::Jira,
                status: "pending".into(),
                last_success_at: None,
                last_attempt_at: None,
                error: None,
                retry_at: None,
                coverage_notes: vec![],
            },
            progress: JiraOptionsProgress {
                started_at: Some(Utc::now() + chrono::Duration::days(1)),
                groups: vec![ChoiceProgress {
                    name: "Verified group".into(),
                    phase: SourcePhase::Completed,
                    completed_items: 2,
                    total_items: Some(2),
                }],
                projects: vec![ChoiceProgress {
                    name: "DEV".into(),
                    phase: SourcePhase::Fetching,
                    completed_items: 1,
                    total_items: Some(2),
                }],
                ..JiraOptionsProgress::new(SourcePhase::Fetching)
            },
        });
        live.options_clock = Some(Instant::now() - std::time::Duration::from_millis(20));
        assert!(live.sample().jira_options.unwrap().progress.elapsed_ms >= 20);
        live.finish_options(SourcePhase::Failed);
        let frozen = live.sample().jira_options.unwrap().progress;
        assert_eq!(frozen.groups[0].phase, SourcePhase::Completed);
        assert_eq!(frozen.projects[0].phase, SourcePhase::Failed);
        assert_eq!(frozen.projects[0].completed_items, 1);
        assert_eq!(
            live.sample().jira_options.unwrap().progress.elapsed_ms,
            frozen.elapsed_ms
        );
        live.enqueue();
        let reset = live.sample().jira_options.unwrap().progress;
        assert_eq!(reset.phase, SourcePhase::Queued);
        assert_eq!(reset.elapsed_ms, 0);
        assert!(reset.groups.is_empty() && reset.projects.is_empty());
        assert!(reset.started_at.is_none() && reset.finished_at.is_none());
    }
}
