use super::{Forge, github::segment, people, references};
use crate::{
    attention::{self, Event, Thread},
    cache::SourceData,
    config::{ForgeConfig, safe_url},
    error::{Error, Result},
    model::*,
    sync::Progress,
    transport::ReadTransport,
};
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};

pub struct Forgejo {
    config: ForgeConfig,
    http: Arc<ReadTransport>,
}
impl Forgejo {
    pub fn new(config: ForgeConfig, http: Arc<ReadTransport>) -> Self {
        Self { config, http }
    }
}
#[derive(Clone, Deserialize)]
struct User {
    // Forgejo represents deleted users with the signed ghost-user ID (-1).
    id: i64,
    login: String,
    #[serde(default)]
    full_name: String,
    html_url: Option<String>,
    #[serde(default)]
    bot: bool,
    #[serde(default)]
    is_bot: bool,
}
impl User {
    fn person(&self, base: &str) -> Person {
        Person {
            id: self.id.to_string(),
            username: self.login.clone(),
            display_name: if self.full_name.is_empty() {
                self.login.clone()
            } else {
                self.full_name.clone()
            },
            url: self.html_url.as_deref().and_then(safe_url).or_else(|| {
                safe_url(&format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    segment(&self.login)
                ))
            }),
        }
    }
    fn bot(&self) -> bool {
        self.bot || self.is_bot || self.login.ends_with("[bot]")
    }
}
#[derive(Deserialize)]
struct Repository {
    full_name: String,
}
#[derive(Deserialize)]
struct SearchIssue {
    number: u64,
    repository: Repository,
}
#[derive(Deserialize)]
struct Pull {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    user: User,
    #[serde(default, deserialize_with = "null_vec")]
    assignees: Vec<User>,
    #[serde(default, deserialize_with = "null_vec")]
    requested_reviewers: Vec<User>,
    #[serde(default, deserialize_with = "null_vec")]
    labels: Vec<Label>,
    created_at: Time,
    updated_at: Time,
    #[serde(default)]
    draft: bool,
}
#[derive(Deserialize)]
struct Label {
    name: String,
}
#[derive(Deserialize)]
struct Comment {
    id: u64,
    body: String,
    user: User,
    created_at: Time,
    html_url: Option<String>,
}
#[derive(Deserialize)]
struct Review {
    id: u64,
    user: Option<User>,
    body: Option<String>,
    state: String,
    submitted_at: Option<Time>,
    #[serde(default)]
    dismissed: bool,
}
#[derive(Deserialize)]
struct ReviewComment {
    id: u64,
    body: String,
    user: User,
    created_at: Time,
    #[serde(default)]
    in_reply_to: Option<u64>,
    #[serde(default)]
    resolved: Option<bool>,
    #[serde(default)]
    resolver: Option<User>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    position: u64,
    #[serde(default)]
    original_position: u64,
    #[serde(default)]
    extra_lines_count: u64,
    html_url: Option<String>,
}
#[derive(Deserialize)]
struct Commit {
    commit: CommitData,
}
#[derive(Deserialize)]
struct CommitData {
    committer: Option<CommitPerson>,
}
#[derive(Deserialize)]
struct CommitPerson {
    date: Option<Time>,
}

#[async_trait]
impl Forge for Forgejo {
    fn source_id(&self) -> &str {
        &self.config.id
    }
    async fn identity(&self) -> Result<Identity> {
        let me: User = self.http.get("user", &[]).await?;
        Ok(Identity {
            source_id: self.config.id.clone(),
            kind: self.config.kind,
            account_id: me.id.to_string(),
            display_name: me.person(&self.config.base_url).display_name,
            username: me.login.clone(),
            profile_url: me.person(&self.config.base_url).url,
        })
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        let me: User = self.http.get("user", &[]).await?;
        progress.discovering();
        // Search account involvement across all accessible repositories, not just
        // /user/repos: a mention or review request can be in an unjoined public repo.
        let mut found = std::collections::BTreeSet::new();
        for filter in ["created", "assigned", "mentioned", "review_requested"] {
            let issues: Vec<SearchIssue> = self
                .http
                .pages(
                    "repos/issues/search",
                    &[
                        ("type", "pulls".into()),
                        ("state", "open".into()),
                        (filter, "true".into()),
                    ],
                )
                .await?;
            for issue in issues {
                if self.config.includes(&issue.repository.full_name) {
                    found.insert((issue.repository.full_name, issue.number));
                }
            }
        }
        progress.fetching(Some(
            u64::try_from(found.len()).map_err(|_| Error::upstream())?,
        ));
        let mut data=SourceData{coverage_notes:vec!["Forgejo/Codeberg discovers account-created, assigned, mentioned and review-requested PRs with paginated global account searches, including private repositories permitted by the token and public repositories the account has not joined. Search indexing, team-only review requests, deleted comments and bot identities not marked by the API may be unavailable. Review threads use the review ID and code location, with resolver metadata when exposed. Requested-review timestamps and description edit history vary across server versions; unknown history is partial and no request timer is invented.".into()],identity:Some(Identity{source_id:self.config.id.clone(),kind:self.config.kind,account_id:me.id.to_string(),display_name:me.person(&self.config.base_url).display_name,username:me.login.clone(),profile_url:me.person(&self.config.base_url).url}),..Default::default()};
        let mut evaluated: Vec<_> = stream::iter(found.into_iter().enumerate())
            .map(|(index, (repository, number))| {
                let me = &me.login;
                let progress = &progress;
                async move {
                    let (owner, name) = repository.split_once('/').ok_or_else(Error::upstream)?;
                    let root = format!("repos/{}/{}", segment(owner), segment(name));
                    let pr = self.pull(&root, owner, name, number, me).await?;
                    progress.item_completed();
                    Ok::<_, Error>((index, pr))
                }
            })
            .buffer_unordered(self.http.limits().max_concurrent_items)
            .try_collect()
            .await?;
        evaluated.sort_by_key(|(index, _)| *index);
        for (_, pr) in evaluated {
            if attention::mentions(&pr.description, &me.login) && !pr.is_mine {
                data.coverage_notes.push(format!(
                        "Description mention on {}: the server does not expose a body-revision timestamp. Inspect the PR directly; no mention timer was fabricated.",
                        pr.url
                    ));
            }
            data.pull_requests.push(pr);
        }
        data.upstream_issues = crate::upstream::discover(
            &self.config,
            &self.http,
            data.identity.as_ref().ok_or_else(Error::upstream)?,
            &progress,
        )
        .await?;
        Ok(data)
    }
    async fn issue(
        &self,
        route: &crate::upstream::IssueRoute,
        identity: &Identity,
    ) -> Result<UpstreamIssue> {
        crate::upstream::fetch(&self.config, &self.http, route, identity).await
    }
}
impl Forgejo {
    async fn pull(
        &self,
        root: &str,
        owner: &str,
        repo: &str,
        number: u64,
        me: &str,
    ) -> Result<PullRequest> {
        // Fetch details; listing responses on older installations omit requested reviewers.
        let p: Pull = self
            .http
            .get(&format!("{root}/pulls/{number}"), &[])
            .await?;
        let mine = p.user.login.eq_ignore_ascii_case(me);
        let url = safe_url(&p.html_url).ok_or_else(Error::upstream)?;
        let comments_path = format!("{root}/issues/{}/comments", p.number);
        let reviews_path = format!("{root}/pulls/{}/reviews", p.number);
        let commits_path = format!("{root}/pulls/{}/commits", p.number);
        let (comments, reviews, commits): (Vec<Comment>, Vec<Review>, Vec<Commit>) = tokio::try_join!(
            self.http.pages(&comments_path, &[]),
            self.http.pages(&reviews_path, &[]),
            self.http.pages(&commits_path, &[]),
        )?;
        let mut flat = Thread {
            id: "conversation".into(),
            url: url.clone(),
            resolved: false,
            events: vec![],
        };
        for c in comments {
            let _ = (c.id, c.html_url);
            flat.events.push(Event {
                at: c.created_at,
                own: c.user.login.eq_ignore_ascii_case(me),
                bot: c.user.bot(),
                opens: reason(&c.body, mine, me),
                excerpt: c.body,
            });
        }
        let mut threads = BTreeMap::<String, Thread>::new();
        let mut request_events = vec![];
        for review in &reviews {
            // Team-only requests have no user in the standard API representation.
            let Some(reviewer) = &review.user else {
                continue;
            };
            if let Some(at) = review.submitted_at.filter(|at| *at >= p.created_at) {
                if review.state == "REQUEST_REVIEW" && reviewer.login.eq_ignore_ascii_case(me) {
                    request_events.push(Event {
                        at,
                        own: false,
                        bot: false,
                        opens: Some(Reason::ReviewRequest),
                        excerpt: "Review requested".into(),
                    });
                } else if reviewer.login.eq_ignore_ascii_case(me) && review.state != "PENDING" {
                    request_events.push(Event {
                        at,
                        own: true,
                        bot: reviewer.bot(),
                        opens: None,
                        excerpt: String::new(),
                    });
                }
                if let Some(body) = &review.body
                    && !reviewer.login.eq_ignore_ascii_case(me)
                    && !body.is_empty()
                {
                    flat.events.push(Event {
                        at,
                        own: reviewer.login.eq_ignore_ascii_case(me),
                        bot: reviewer.bot(),
                        opens: reason(body, mine, me),
                        excerpt: body.clone(),
                    });
                }
            }
            if review.state == "REQUEST_REVIEW" || review.state == "PENDING" {
                continue;
            }
            let comments: Vec<ReviewComment> = self
                .http
                .pages(
                    &format!("{root}/pulls/{}/reviews/{}/comments", p.number, review.id),
                    &[],
                )
                .await?;
            for c in comments {
                add_review_comment(&mut threads, review.id, c, &url, mine, me);
            }
            let _ = review.dismissed;
        }
        let mut attention_threads: Vec<_> = threads.into_values().collect();
        attention_threads.push(flat);
        if p.requested_reviewers
            .iter()
            .any(|u| u.login.eq_ignore_ascii_case(me))
            || request_events
                .iter()
                .any(|e| e.opens == Some(Reason::ReviewRequest))
        {
            // A fresh request supersedes earlier requests, not unrelated code threads.
            if let Some(latest) = request_events
                .iter()
                .filter(|e| e.opens == Some(Reason::ReviewRequest))
                .map(|e| e.at)
                .max()
            {
                request_events.retain(|e| e.opens.is_none() || e.at == latest);
            }
            attention_threads.push(Thread {
                id: "review-request".into(),
                url: url.clone(),
                resolved: false,
                events: request_events,
            });
        }
        let description = p.body.unwrap_or_default();
        let attention = attention::compute(attention_threads, true);
        Ok(PullRequest {
            id: format!("{}:{owner}/{repo}:{}", self.config.id, p.number),
            source_id: self.config.id.clone(),
            forge: self.config.kind,
            number: p.number,
            owner: owner.into(),
            repository: repo.into(),
            title: p.title.clone(),
            url,
            description: description.clone(),
            state: if p.draft || p.title.starts_with("WIP:") || p.title.starts_with("[WIP]") {
                "draft"
            } else {
                "ready"
            }
            .into(),
            author: p.user.person(&self.config.base_url),
            assignees: people(
                p.assignees
                    .iter()
                    .map(|u| u.person(&self.config.base_url))
                    .collect(),
                me,
            ),
            reviewers: people(
                p.requested_reviewers
                    .iter()
                    .chain(reviews.iter().filter_map(|r| r.user.as_ref()))
                    .map(|u| u.person(&self.config.base_url))
                    .collect(),
                me,
            ),
            labels: p.labels.into_iter().map(|l| l.name).collect(),
            created_at: p.created_at,
            updated_at: p.updated_at,
            last_commit_at: commits
                .into_iter()
                .filter_map(|c| c.commit.committer.and_then(|c| c.date))
                .max(),
            linked_issue_keys: references(&format!("{}\n{description}", p.title)),
            is_mine: mine,
            attention,
        })
    }
}

// Forgejo uses JSON null, not [], for several unpopulated user/label lists.
fn null_vec<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn add_review_comment(
    threads: &mut BTreeMap<String, Thread>,
    review_id: u64,
    c: ReviewComment,
    url: &str,
    mine: bool,
    me: &str,
) {
    // Forgejo replies share the original review and code location; its standard
    // API does not expose in_reply_to. Do not merge separate reviews on one line.
    let id = if !c.path.is_empty() && (c.position > 0 || c.original_position > 0) {
        format!(
            "review:{review_id}:{}:{}:{}:{}",
            c.path, c.position, c.original_position, c.extra_lines_count
        )
    } else {
        format!(
            "review:{review_id}:comment:{}",
            c.in_reply_to.filter(|id| *id > 0).unwrap_or(c.id)
        )
    };
    let resolved = c.resolved == Some(true) || c.resolver.is_some();
    let thread = threads.entry(id.clone()).or_insert_with(|| Thread {
        id,
        url: c
            .html_url
            .as_deref()
            .and_then(safe_url)
            .unwrap_or_else(|| url.to_owned()),
        resolved: true,
        events: vec![],
    });
    thread.resolved &= resolved;
    thread.events.push(Event {
        at: c.created_at,
        own: c.user.login.eq_ignore_ascii_case(me),
        bot: c.user.bot(),
        // A newly posted unresolved comment can reopen a resolved location, but
        // older explicitly resolved comments must not restart its waiting clock.
        opens: (!resolved).then(|| reason(&c.body, mine, me)).flatten(),
        excerpt: c.body,
    });
}

fn reason(body: &str, mine: bool, me: &str) -> Option<Reason> {
    if attention::mentions(body, me) {
        Some(Reason::PrMention)
    } else if mine {
        Some(Reason::AuthorFeedback)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn comment(id: u64, user: &str, day: u32, line: u64, resolved: bool) -> ReviewComment {
        serde_json::from_value(json!({
            "id": id, "body": "Please check this", "created_at": format!("2026-01-{day:02}T00:00:00Z"),
            "user": {"id": id, "login": user, "html_url": null},
            "resolver": if resolved { json!({"id": 9, "login": "resolver", "html_url": null}) } else { json!(null) },
            "path": "src/cache.rs", "position": line, "original_position": 0,
            "html_url": format!("https://forge.example/pulls/1#issuecomment-{id}")
        })).unwrap()
    }

    #[test]
    fn standard_forgejo_reply_clears_only_its_code_location() {
        let mut threads = BTreeMap::new();
        let url = "https://forge.example/pulls/1";
        for c in [
            comment(1, "other", 1, 10, false),
            comment(2, "me", 3, 10, false),
            comment(3, "other", 2, 20, false),
        ] {
            add_review_comment(&mut threads, 7, c, url, true, "me");
        }
        let a = attention::compute(threads.into_values().collect(), true).unwrap();
        assert_eq!(a.threads.len(), 1);
        assert!(a.threads[0].id.contains(":20:"));
        assert_eq!(a.waiting_since.to_rfc3339(), "2026-01-02T00:00:00+00:00");
    }

    #[test]
    fn resolver_clears_and_new_unresolved_comment_reopens_at_new_timestamp() {
        let mut threads = BTreeMap::new();
        let url = "https://forge.example/pulls/1";
        add_review_comment(
            &mut threads,
            7,
            comment(1, "other", 1, 10, true),
            url,
            true,
            "me",
        );
        assert!(attention::compute(threads.values().cloned().collect(), true).is_none());
        add_review_comment(
            &mut threads,
            7,
            comment(2, "other", 4, 10, false),
            url,
            true,
            "me",
        );
        let a = attention::compute(threads.into_values().collect(), true).unwrap();
        assert_eq!(a.waiting_since.to_rfc3339(), "2026-01-04T00:00:00+00:00");
    }

    #[test]
    fn review_locations_do_not_merge_unrelated_reviews() {
        let mut threads = BTreeMap::new();
        let url = "https://forge.example/pulls/1";
        add_review_comment(
            &mut threads,
            7,
            comment(1, "other", 1, 10, false),
            url,
            true,
            "me",
        );
        add_review_comment(
            &mut threads,
            8,
            comment(2, "me", 3, 10, false),
            url,
            true,
            "me",
        );
        let a = attention::compute(threads.into_values().collect(), true).unwrap();
        assert_eq!(a.threads.len(), 1);
        assert!(a.threads[0].id.starts_with("review:7:"));
    }

    #[test]
    fn deleted_user_ids_are_preserved_without_failing_the_source() {
        let user: User =
            serde_json::from_value(json!({"id": -1, "login": "Ghost", "html_url": null})).unwrap();
        assert_eq!(user.person("https://forge.example").id, "-1");
    }
}
