//! Attention is computed from upstream events, never from local discovery or commits.
use crate::model::{Attention, AttentionThread, Reason, Time};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Event {
    pub at: Time,
    pub own: bool,
    pub bot: bool,
    pub opens: Option<Reason>,
    pub excerpt: String,
}
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: String,
    pub url: String,
    pub resolved: bool,
    pub events: Vec<Event>,
}

pub fn compute(threads: Vec<Thread>, partial: bool) -> Option<Attention> {
    let mut unanswered = Vec::new();
    for thread in threads {
        if thread.resolved {
            continue;
        }
        // Equal timestamps are not a demonstrably subsequent reply. Preserve attention.
        let cleared = thread
            .events
            .iter()
            .filter(|e| e.own && !e.bot)
            .map(|e| e.at)
            .max();
        let mut by_reason = BTreeMap::new();
        for e in thread.events {
            if e.bot || e.own || cleared.is_some_and(|at| e.at < at) {
                continue;
            }
            if let Some(reason) = e.opens {
                let entry = by_reason.entry(reason).or_insert((e.at, e.excerpt.clone()));
                if e.at < entry.0 {
                    *entry = (e.at, e.excerpt);
                }
            }
        }
        // A stream has one timer; choose a deterministic reason if its earliest events tie.
        if let Some((reason, (at, excerpt))) =
            by_reason.into_iter().min_by_key(|(r, (at, _))| (*at, *r))
        {
            unanswered.push(AttentionThread {
                id: thread.id,
                url: thread.url,
                reason,
                waiting_since: at,
                excerpt: excerpt.chars().take(300).collect(),
            });
        }
    }
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
        coverage: if partial { "partial" } else { "complete" }.into(),
    })
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
    fn mention_boundaries() {
        assert!(mentions("Hi @alex!", "alex"));
        assert!(!mentions("a@alex.example @alexander", "alex"));
    }
}
