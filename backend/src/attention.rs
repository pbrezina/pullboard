//! Attention is computed from upstream events, never from local discovery or commits.
use crate::model::{Attention, AttentionThread, Reason, Time};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct Event {
    pub at: Time,
    pub own: bool,
    pub bot: bool,
    pub opens: Option<Reason>,
    /// A human follow-up in an identifiable review discussion, not an opener.
    pub follow_up: bool,
    pub excerpt: String,
}
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: String,
    pub url: String,
    pub resolved: bool,
    pub events: Vec<Event>,
}

impl Thread {
    fn latest_own_reply_at(&self) -> Option<Time> {
        self.events
            .iter()
            .filter(|e| e.own && !e.bot)
            .map(|e| e.at)
            .max()
    }

    /// Count a reply in the general conversation as an answer to this review request
    /// without copying any of that conversation's attention-opening events.
    pub(crate) fn accept_conversation_reply(&mut self, conversation: &Self) {
        if let Some(at) = conversation.latest_own_reply_at() {
            self.events.push(Event {
                at,
                own: true,
                bot: false,
                opens: None,
                follow_up: false,
                excerpt: String::new(),
            });
        }
    }
}

pub fn compute(threads: Vec<Thread>, partial: bool) -> Option<Attention> {
    let mut unanswered = Vec::new();
    for thread in threads {
        if thread.resolved {
            continue;
        }
        // Equal timestamps are not a demonstrably subsequent reply. Preserve attention.
        let cleared = thread.latest_own_reply_at();
        let mut oldest = None;
        let mut latest = None;
        for e in thread.events {
            if e.bot || e.own || cleared.is_some_and(|at| e.at < at) {
                continue;
            }
            if let Some(reason) = e.opens {
                if oldest
                    .as_ref()
                    .is_none_or(|(at, r, _)| (e.at, reason) < (*at, *r))
                {
                    oldest = Some((e.at, reason, e.excerpt));
                }
            }
            if e.opens.is_some() || e.follow_up {
                latest = latest.max(Some(e.at));
            }
        }
        // A stream has one timer; choose a deterministic reason if its earliest events tie.
        if let Some((at, reason, excerpt)) = oldest {
            unanswered.push(AttentionThread {
                id: thread.id,
                url: thread.url,
                reason,
                waiting_since: at,
                latest_unanswered_at: latest,
                excerpt: excerpt.chars().take(300).collect(),
            });
        }
    }
    aggregate(
        unanswered,
        if partial { "partial" } else { "complete" }.into(),
    )
}

fn aggregate(mut unanswered: Vec<AttentionThread>, coverage: String) -> Option<Attention> {
    unanswered.sort_by(|a, b| {
        a.waiting_since
            .cmp(&b.waiting_since)
            .then_with(|| a.id.cmp(&b.id))
    });
    let waiting_since = unanswered.first()?.waiting_since;
    let reasons = unanswered
        .iter()
        .map(|t| t.reason)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(Attention {
        waiting_since,
        reasons,
        threads: unanswered,
        coverage,
    })
}

fn thread_retained(thread: &AttentionThread, cutoff: Time) -> bool {
    // Legacy cached data does not have a verified latest timestamp. Do not
    // conclude its last event was old from the oldest event alone.
    thread.latest_unanswered_at.is_none_or(|at| at >= cutoff)
}

/// Returns true if this card had attention but all of it expired.
pub(crate) fn expire(attention: &mut Option<Attention>, cutoff: Time) -> bool {
    let Some(current) = attention.as_mut() else {
        return false;
    };
    let before = current.threads.len();
    current.threads.retain(|t| thread_retained(t, cutoff));
    if current.threads.len() != before {
        *attention = aggregate(
            std::mem::take(&mut current.threads),
            std::mem::take(&mut current.coverage),
        );
    }
    attention.is_none()
}

pub fn mentions(text: &str, username: &str) -> bool {
    let pattern = format!(r"(?i)(?:^|[^\w@])@{}(?:$|[^\w-])", regex::escape(username));
    regex::Regex::new(&pattern).is_ok_and(|r| r.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    fn event(t: i64, own: bool) -> Event {
        Event {
            at: Utc.timestamp_opt(t, 0).unwrap(),
            own,
            bot: false,
            opens: Some(Reason::AuthorFeedback),
            follow_up: false,
            excerpt: "feedback".into(),
        }
    }
    fn thread(id: &str, events: Vec<Event>) -> Thread {
        Thread {
            id: id.into(),
            url: "https://forge/pr".into(),
            resolved: false,
            events,
        }
    }
    #[test]
    fn reply_clears_only_its_thread_and_push_is_not_an_event() {
        let a = compute(
            vec![
                thread("flat", vec![event(1, false), event(3, true)]),
                thread("review", vec![event(2, false)]),
            ],
            false,
        )
        .unwrap();
        assert_eq!(a.threads.len(), 1);
        assert_eq!(a.threads[0].id, "review");
        assert_eq!(a.waiting_since.timestamp(), 2);
    }
    #[test]
    fn review_submission_only_clears_request_stream() {
        let a = compute(
            vec![
                thread("request", vec![event(1, false), event(5, true)]),
                thread("unrelated", vec![event(2, false)]),
            ],
            true,
        )
        .unwrap();
        assert_eq!(a.threads[0].id, "unrelated");
        assert_eq!(a.coverage, "partial");
    }
    #[test]
    fn bots_resolution_reopen_and_oldest() {
        let mut bot = event(0, false);
        bot.bot = true;
        let a = compute(
            vec![
                thread("a", vec![bot, event(2, false), event(3, false)]),
                thread("b", vec![event(1, false)]),
            ],
            false,
        )
        .unwrap();
        assert_eq!(a.waiting_since.timestamp(), 1);
        let mut t = thread("x", vec![event(1, false)]);
        t.resolved = true;
        assert!(compute(vec![t], false).is_none());
        let a = compute(
            vec![thread(
                "x",
                vec![event(1, false), event(2, true), event(4, false)],
            )],
            false,
        )
        .unwrap();
        assert_eq!(a.waiting_since.timestamp(), 4);
    }
    #[test]
    fn equal_timestamps_are_not_proven_replies_and_sort_stably() {
        let a = compute(
            vec![
                thread("z", vec![event(1, false), event(1, true)]),
                thread("a", vec![event(1, false)]),
            ],
            false,
        )
        .unwrap();
        assert_eq!(a.threads[0].id, "a");
        assert_eq!(a.threads.len(), 2);
    }
    #[test]
    fn fresh_follow_up_in_unanswered_discussion_preserves_its_oldest_event() {
        let mut follow_up = event(2, false);
        follow_up.opens = None;
        follow_up.follow_up = true;
        let mut bot = follow_up.clone();
        bot.at = event(3, false).at;
        bot.bot = true;
        let mut unrelated = event(4, false);
        unrelated.opens = None;
        assert!(compute(vec![thread("review", vec![follow_up.clone()])], false).is_none());
        let a = compute(
            vec![thread(
                "review",
                vec![unrelated, follow_up, bot, event(1, false)],
            )],
            false,
        )
        .unwrap();
        assert_eq!(a.waiting_since.timestamp(), 1);
        assert_eq!(a.threads[0].latest_unanswered_at.unwrap().timestamp(), 2);
    }

    #[test]
    fn older_cached_thread_deserializes_without_inventing_freshness() {
        let a = compute(vec![thread("review", vec![event(1, false)])], false).unwrap();
        let mut json = serde_json::to_value(&a.threads[0]).unwrap();
        json.as_object_mut().unwrap().remove("latest_unanswered_at");
        let cached: AttentionThread = serde_json::from_value(json).unwrap();
        assert_eq!(cached.latest_unanswered_at, None);
        assert_eq!(cached.waiting_since, a.waiting_since);
    }
    #[test]
    fn mention_boundaries() {
        assert!(mentions("Hi @alex!", "alex"));
        assert!(!mentions("a@alex.example @alexander", "alex"));
    }
}
