//! Process-local admission, shared conservatively by HTTP origin, never by secret.
use crate::{
    config::Upstream,
    error::{Error, Result},
    model::Time,
};
use chrono::{DateTime, Utc};
use http::{HeaderMap, StatusCode};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
    time::Instant,
};
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Bucket {
    General,
    GithubCore,
    GithubSearch,
    GithubGraphql,
}
impl Bucket {
    fn label(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::GithubCore => "github_core",
            Self::GithubSearch => "github_search",
            Self::GithubGraphql => "github_graphql",
        }
    }
    fn reported_bucket(self, headers: &HeaderMap) -> Self {
        if self == Self::General {
            return self;
        }
        match headers
            .get("x-ratelimit-resource")
            .and_then(|h| h.to_str().ok())
        {
            Some("core") => Self::GithubCore,
            Some("search" | "code_search") => Self::GithubSearch,
            Some("graphql") => Self::GithubGraphql,
            _ => self,
        }
    }
}

struct Primary {
    remaining: u64,
    reset: Time,
}
struct State {
    limit: usize,
    active: usize,
    requests: u64,
    blocked: u64,
    secondary: Option<Time>,
    primary: BTreeMap<Bucket, Primary>,
}
pub(crate) struct RequestBudget {
    state: Mutex<State>,
    changed: Notify,
}
pub(crate) struct RequestPermit {
    budget: Arc<RequestBudget>,
    started: Instant,
    bucket: Bucket,
}

type Registry = BTreeMap<String, Weak<RequestBudget>>;
static BUDGETS: OnceLock<Mutex<Registry>> = OnceLock::new();

impl RequestBudget {
    pub(crate) fn shared(base: &str, limits: &Upstream) -> anyhow::Result<Arc<Self>> {
        let url = url::Url::parse(base).map_err(|_| anyhow::anyhow!("invalid upstream origin"))?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "invalid upstream origin"
        );
        let mut registry = BUDGETS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.retain(|_, budget| budget.strong_count() > 0);
        let key = url.origin().ascii_serialization();
        if let Some(budget) = registry.get(&key).and_then(Weak::upgrade) {
            // In practice all instances use the same YAML bounds. If callers differ,
            // lower the common ceiling, never silently raise another instance's cap.
            let mut state = budget.state();
            state.limit = state.limit.min(limits.max_concurrent_requests.clamp(1, 16));
            drop(state);
            return Ok(budget);
        }
        let budget = Arc::new(Self {
            state: Mutex::new(State {
                limit: limits.max_concurrent_requests.clamp(1, 16),
                active: 0,
                requests: 0,
                blocked: 0,
                secondary: None,
                primary: BTreeMap::new(),
            }),
            changed: Notify::new(),
        });
        registry.insert(key, Arc::downgrade(&budget));
        Ok(budget)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn blocked_at(state: &State, bucket: Bucket, now: Time) -> Option<Time> {
        state
            .secondary
            .into_iter()
            .chain(
                state
                    .primary
                    .get(&bucket)
                    .filter(|p| p.remaining == 0)
                    .map(|p| p.reset),
            )
            .filter(|at| *at > now)
            .max()
    }

    pub(crate) async fn acquire(self: &Arc<Self>, bucket: Bucket) -> Result<RequestPermit> {
        let queued = Instant::now();
        loop {
            // Register before inspecting state so completion/backoff cannot be lost.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.state();
                let now = Utc::now();
                if let Some(at) = Self::blocked_at(&state, bucket, now) {
                    state.blocked = state.blocked.saturating_add(1);
                    tracing::debug!(
                        bucket = bucket.label(),
                        blocked = state.blocked,
                        "upstream request suppressed"
                    );
                    return Err(Error::backoff(at));
                }
                if state.active < state.limit {
                    state.active += 1;
                    // Recheck at permit admission, not merely before waiting. State
                    // and reservation share one lock with rate-limit publication.
                    if let Some(primary) = state.primary.get_mut(&bucket)
                        && primary.reset > now
                    {
                        primary.remaining = primary.remaining.saturating_sub(1);
                    }
                    state.requests = state.requests.saturating_add(1);
                    tracing::debug!(
                        bucket = bucket.label(),
                        active = state.active,
                        requests = state.requests,
                        queued_ms = queued.elapsed().as_millis(),
                        "upstream request admitted"
                    );
                    return Ok(RequestPermit {
                        budget: Arc::clone(self),
                        started: Instant::now(),
                        bucket,
                    });
                }
            }
            notified.await;
        }
    }

    fn primary(&self, bucket: Bucket, remaining: u64, reset: Time) {
        let mut state = self.state();
        match state.primary.get_mut(&bucket) {
            Some(previous) if previous.reset > Utc::now() && reset > Utc::now() => {
                // Responses can finish out of order. A late success must never
                // replenish an exhausted window (nor clear secondary backoff).
                // Independent credentials on the same origin can have overlapping
                // windows: conservatively retain both the lower quota and later reset.
                previous.remaining = previous.remaining.min(remaining);
                previous.reset = previous.reset.max(reset);
            }
            Some(previous) if previous.reset > reset => {}
            _ => {
                state.primary.insert(bucket, Primary { remaining, reset });
            }
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn throttle(&self, bucket: Bucket, at: Time, secondary: bool) -> Error {
        let mut state = self.state();
        let deadline = if secondary {
            let deadline = state.secondary.map_or(at, |previous| previous.max(at));
            state.secondary = Some(deadline);
            deadline
        } else {
            let primary = state.primary.entry(bucket).or_insert(Primary {
                remaining: 0,
                reset: at,
            });
            primary.remaining = 0;
            primary.reset = primary.reset.max(at);
            primary.reset
        };
        drop(state);
        self.changed.notify_waiters();
        Error::backoff(deadline)
    }

    /// Publish headers before consuming a body so queued work sees the signal.
    pub(crate) fn observe_headers(
        &self,
        bucket: Bucket,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Option<Error> {
        let bucket = bucket.reported_bucket(headers);
        let remaining = remaining(headers);
        if let Some(remaining) = remaining {
            if let Some(reset) = reset_at(headers) {
                self.primary(bucket, remaining, reset);
            } else if remaining == 0 {
                self.primary(bucket, 0, retry_at(headers));
            }
        }
        rate_error(status, headers).map(|error| {
            self.throttle(
                bucket,
                error.retry_at.unwrap_or_else(|| retry_at(headers)),
                headers.contains_key("retry-after") || remaining != Some(0),
            )
        })
    }

    /// GitHub secondary/abuse limits are also reported as 403 JSON or GraphQL 200.
    pub(crate) fn observe_body(
        &self,
        bucket: Bucket,
        status: StatusCode,
        headers: &HeaderMap,
        bytes: &[u8],
    ) -> Option<Error> {
        if !matches!(
            bucket,
            Bucket::GithubCore | Bucket::GithubSearch | Bucket::GithubGraphql
        ) {
            return None;
        }
        if bucket != Bucket::GithubGraphql
            && !matches!(
                status,
                StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
            )
        {
            return None;
        }
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if bucket == Bucket::GithubGraphql {
            if let Some(rate) = value.pointer("/data/rateLimit")
                && let (Some(remaining), Some(reset)) = (
                    rate.get("remaining").and_then(Value::as_u64),
                    rate.get("resetAt").and_then(Value::as_str),
                )
                && let Ok(reset) = DateTime::parse_from_rfc3339(reset)
            {
                self.primary(bucket, remaining, reset.with_timezone(&Utc));
            }
            if let Some(errors) = value.get("errors").and_then(Value::as_array) {
                let limited = errors.iter().any(|e| {
                    [
                        e.get("type"),
                        e.pointer("/extensions/code"),
                        e.pointer("/extensions/type"),
                    ]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|code| {
                        matches!(
                            code,
                            "RATE_LIMITED" | "RATE_LIMIT" | "THROTTLED" | "ABUSE_DETECTED"
                        )
                    }) || e
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(rate_message)
                });
                if limited {
                    let secondary = errors.iter().any(|e| {
                        e.get("message")
                            .and_then(Value::as_str)
                            .is_some_and(secondary_message)
                    });
                    let at = Self::blocked_at(&self.state(), bucket, Utc::now())
                        .unwrap_or_else(|| retry_at(headers));
                    return Some(self.throttle(
                        bucket,
                        at,
                        secondary || remaining(headers) != Some(0),
                    ));
                }
            }
        } else if matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) && let Some(message) = value.get("message").and_then(Value::as_str)
            && rate_message(message)
        {
            let bucket = bucket.reported_bucket(headers);
            let at = Self::blocked_at(&self.state(), bucket, Utc::now())
                .unwrap_or_else(|| retry_at(headers));
            return Some(self.throttle(
                bucket,
                at,
                secondary_message(message) || remaining(headers) != Some(0),
            ));
        }
        None
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        let mut state = self.budget.state();
        state.active = state.active.saturating_sub(1);
        tracing::debug!(
            bucket = self.bucket.label(),
            active = state.active,
            elapsed_ms = self.started.elapsed().as_millis(),
            "upstream request finished"
        );
        if state.active == 0 {
            tracing::info!(
                bucket = self.bucket.label(),
                requests = state.requests,
                suppressed = state.blocked,
                "upstream request budget idle"
            );
        }
        drop(state);
        self.budget.changed.notify_waiters();
    }
}

fn secondary_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "secondary rate",
        "abuse",
        "submitted too quickly",
        "please wait a few minutes before you try again",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}
fn rate_message(message: &str) -> bool {
    secondary_message(message) || {
        let message = message.to_ascii_lowercase();
        ["rate limit", "rate-limit", "rate_limit"]
            .iter()
            .any(|needle| message.contains(needle))
    }
}
fn remaining(headers: &HeaderMap) -> Option<u64> {
    ["x-ratelimit-remaining", "ratelimit-remaining"]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok()?.parse().ok())
}
fn reset_at(headers: &HeaderMap) -> Option<Time> {
    ["x-ratelimit-reset", "ratelimit-reset"]
        .iter()
        .find_map(|name| {
            let value = headers.get(*name)?.to_str().ok()?;
            value
                .parse()
                .ok()
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
                .or_else(|| {
                    DateTime::parse_from_rfc3339(value)
                        .ok()
                        .map(|at| at.with_timezone(&Utc))
                })
        })
}
fn retry_at(headers: &HeaderMap) -> Time {
    let now = Utc::now();
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.parse::<i64>()
                .ok()
                .and_then(|s| chrono::Duration::try_seconds(s.max(1)))
                .and_then(|delay| now.checked_add_signed(delay))
                .or_else(|| httpdate::parse_http_date(v).ok().map(DateTime::<Utc>::from))
        })
        .or_else(|| reset_at(headers))
        .unwrap_or(now + chrono::Duration::seconds(60))
        .max(now + chrono::Duration::seconds(1))
}
pub(crate) fn rate_error(status: StatusCode, headers: &HeaderMap) -> Option<Error> {
    (status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && (headers.contains_key("retry-after") || remaining(headers) == Some(0))))
    .then(|| Error::backoff(retry_at(headers)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn budget() -> Arc<RequestBudget> {
        Arc::new(RequestBudget {
            state: Mutex::new(State {
                limit: 1,
                active: 0,
                requests: 0,
                blocked: 0,
                secondary: None,
                primary: BTreeMap::new(),
            }),
            changed: Notify::new(),
        })
    }

    #[tokio::test]
    async fn queued_admission_rechecks_and_wakes_on_backoff_without_permit_release() {
        let budget = budget();
        let held = budget.acquire(Bucket::GithubCore).await.unwrap();
        let queued = budget.acquire(Bucket::GithubSearch);
        tokio::pin!(queued);
        assert!(futures_util::poll!(&mut queued).is_pending());
        let at = Utc::now() + chrono::Duration::seconds(120);
        budget.throttle(Bucket::GithubCore, at, true);
        let error = queued.await.err().unwrap();
        assert_eq!(error.retry_at, Some(at));
        assert_eq!(budget.state().active, 1);
        drop(held);
        assert_eq!(budget.state().active, 0);
    }

    #[tokio::test]
    async fn shorter_racing_throttle_and_success_never_clear_shared_backoff() {
        let budget = budget();
        let long = Utc::now() + chrono::Duration::seconds(300);
        let short = Utc::now() + chrono::Duration::seconds(60);
        budget.throttle(Bucket::GithubGraphql, long, true);
        budget.throttle(Bucket::GithubCore, short, true);
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("5000"));
        headers.insert(
            "x-ratelimit-reset",
            HeaderValue::from_str(&long.timestamp().to_string()).unwrap(),
        );
        assert!(
            budget
                .observe_headers(Bucket::GithubCore, StatusCode::OK, &headers)
                .is_none()
        );
        let error = budget.acquire(Bucket::GithubSearch).await.err().unwrap();
        assert_eq!(error.retry_at, Some(long));
    }

    #[tokio::test]
    async fn late_primary_success_cannot_replenish_exhausted_window() {
        let budget = budget();
        let at = Utc::now() + chrono::Duration::seconds(120);
        budget.primary(Bucket::GithubSearch, 0, at);
        budget.primary(Bucket::GithubSearch, 10, at);
        assert_eq!(
            budget
                .acquire(Bucket::GithubSearch)
                .await
                .err()
                .unwrap()
                .retry_at,
            Some(at)
        );
        budget.acquire(Bucket::GithubCore).await.unwrap();
    }

    #[tokio::test]
    async fn overlapping_credential_windows_cannot_replenish_or_hide_exhaustion() {
        let budget = budget();
        let earlier = Utc::now() + chrono::Duration::seconds(120);
        let later = earlier + chrono::Duration::seconds(60);
        budget.primary(Bucket::GithubSearch, 1, later);
        budget.primary(Bucket::GithubSearch, 0, earlier);
        budget.primary(Bucket::GithubSearch, 100, later);
        let error = budget.acquire(Bucket::GithubSearch).await.err().unwrap();
        assert_eq!(error.retry_at, Some(later));
    }

    #[test]
    fn retry_after_dates_and_long_delays_are_not_shortened() {
        let at = Utc::now() + chrono::Duration::days(2);
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            HeaderValue::from_str(&httpdate::fmt_http_date(at.into())).unwrap(),
        );
        assert_eq!(retry_at(&headers).timestamp(), at.timestamp());
        headers.insert("retry-after", HeaderValue::from_static("172800"));
        assert!(retry_at(&headers) > Utc::now() + chrono::Duration::seconds(172795));
        headers.insert(
            "retry-after",
            HeaderValue::from_static("9223372036854775807"),
        );
        assert!(retry_at(&headers) > Utc::now());
    }

    #[tokio::test]
    async fn jira_rfc3339_reset_headers_block_until_the_reported_deadline() {
        let budget = budget();
        let at = Utc::now() + chrono::Duration::seconds(300);
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        headers.insert(
            "x-ratelimit-reset",
            HeaderValue::from_str(&at.to_rfc3339()).unwrap(),
        );
        let error = budget
            .observe_headers(Bucket::General, StatusCode::TOO_MANY_REQUESTS, &headers)
            .unwrap();
        assert_eq!(error.retry_at, Some(at));
        let queued = budget.acquire(Bucket::General).await.err().unwrap();
        assert_eq!(queued.retry_at, Some(at));
    }
}
