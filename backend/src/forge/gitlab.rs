use super::{Forge, people, references};
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
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct Gitlab {
    config: ForgeConfig,
    http: Arc<ReadTransport>,
}
impl Gitlab {
    pub fn new(config: ForgeConfig, http: Arc<ReadTransport>) -> Self {
        Self { config, http }
    }
}
#[derive(Clone, Deserialize)]
struct User {
    id: u64,
    username: String,
    name: String,
    web_url: Option<String>,
    #[serde(default)]
    bot: bool,
}
impl User {
    fn person(&self) -> Person {
        Person {
            id: self.id.to_string(),
            username: self.username.clone(),
            display_name: self.name.clone(),
            url: self.web_url.as_deref().and_then(safe_url),
        }
    }
    fn bot(&self) -> bool {
        self.bot
            || self.username.ends_with("[bot]")
            || self.username.starts_with("project_") && self.username.contains("_bot_")
    }
}
#[derive(Clone, Deserialize)]
struct MergeRequest {
    iid: u64,
    project_id: u64,
    title: String,
    description: Option<String>,
    web_url: String,
    author: User,
    #[serde(default)]
    assignees: Vec<User>,
    #[serde(default)]
    reviewers: Vec<User>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    work_in_progress: bool,
    created_at: Time,
    updated_at: Time,
    state: String,
}
#[derive(Deserialize)]
struct Project {
    path_with_namespace: String,
}
#[derive(Deserialize)]
struct Todo {
    target_type: String,
    project: TodoProject,
    target: TodoTarget,
}
#[derive(Deserialize)]
struct TodoProject {
    id: u64,
}
#[derive(Deserialize)]
struct TodoTarget {
    iid: Option<u64>,
    state: Option<String>,
}
#[derive(Deserialize)]
struct Discussion {
    id: String,
    individual_note: bool,
    notes: Vec<Note>,
}
#[derive(Deserialize)]
struct Note {
    id: u64,
    body: String,
    author: User,
    created_at: Time,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    resolvable: bool,
    #[serde(default)]
    resolved: bool,
}
#[derive(Deserialize)]
struct Commit {
    committed_date: Option<Time>,
}

#[async_trait]
impl Forge for Gitlab {
    fn source_id(&self) -> &str {
        &self.config.id
    }
    async fn identity(&self) -> Result<Identity> {
        let me: User = self.http.get("user", &[]).await?;
        Ok(Identity {
            source_id: self.config.id.clone(),
            kind: self.config.kind,
            account_id: me.id.to_string(),
            display_name: me.name.clone(),
            username: me.username.clone(),
            profile_url: me.person().url,
        })
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData> {
        let me: User = self.http.get("user", &[]).await?;
        progress.discovering();
        let mut found = BTreeMap::new();
        for filter in ["author_id", "reviewer_id", "assignee_id"] {
            let mrs: Vec<MergeRequest> = self
                .http
                .pages(
                    "merge_requests",
                    &[
                        ("scope", "all".into()),
                        ("state", "opened".into()),
                        (filter, me.id.to_string()),
                    ],
                )
                .await?;
            for mr in mrs {
                found.insert((mr.project_id, mr.iid), mr);
            }
        }
        let mut todo_candidates = BTreeSet::new();
        for state in ["pending", "done"] {
            let todos: Vec<Todo> = self
                .http
                .pages(
                    "todos",
                    &[("type", "MergeRequest".into()), ("state", state.into())],
                )
                .await?;
            for todo in todos {
                if todo.target_type != "MergeRequest"
                    || todo.target.state.as_deref().is_some_and(|s| s != "opened")
                {
                    continue;
                }
                if let Some(iid) = todo.target.iid
                    && !found.contains_key(&(todo.project.id, iid))
                {
                    todo_candidates.insert((todo.project.id, iid));
                }
            }
        }
        let additional: BTreeMap<_, MergeRequest> = stream::iter(todo_candidates)
            .map(|(project_id, iid)| async move {
                let mr = self
                    .http
                    .get(&format!("projects/{project_id}/merge_requests/{iid}"), &[])
                    .await?;
                Ok::<_, Error>(((project_id, iid), mr))
            })
            .buffer_unordered(self.http.limits().max_concurrent_items)
            .try_collect()
            .await?;
        // Account searches and both to-do states can reference the same MR. Restore
        // stable key order before hydration and never count these discovery reads.
        for mr in additional.into_values() {
            if mr.state == "opened" {
                found.insert((mr.project_id, mr.iid), mr);
            }
        }
        let mut data=SourceData{coverage_notes:vec!["GitLab discovers open merge requests authored by, assigned to, or requesting review from the account, plus accessible account to-do references. Mention discovery depends on retained to-dos and permissions. Review-request history is inferred only from timestamped system notes, not MR updates; unavailable history has no fabricated timer.".into()],identity:Some(Identity{source_id:self.config.id.clone(),kind:Kind::Gitlab,account_id:me.id.to_string(),display_name:me.name.clone(),username:me.username.clone(),profile_url:me.person().url}),..Default::default()};
        let project_ids: BTreeSet<_> = found.keys().map(|(project_id, _)| *project_id).collect();
        let projects: BTreeMap<_, _> = stream::iter(project_ids)
            .map(|project_id| async move {
                let project: Project = self
                    .http
                    .get(&format!("projects/{project_id}"), &[])
                    .await?;
                Ok::<_, Error>((project_id, project.path_with_namespace))
            })
            .buffer_unordered(self.http.limits().max_concurrent_items)
            .try_collect()
            .await?;
        let mut candidates = Vec::new();
        for ((project_id, _), mr) in found {
            let path = projects.get(&project_id).ok_or_else(Error::upstream)?;
            if self.config.includes(path) {
                candidates.push((mr, path.clone()));
            }
        }
        progress.fetching(Some(
            u64::try_from(candidates.len()).map_err(|_| Error::upstream())?,
        ));
        let mut evaluated: Vec<_> = stream::iter(candidates.into_iter().enumerate())
            .map(|(index, (mr, path))| {
                let me = &me.username;
                let progress = &progress;
                async move {
                    let mut notes = vec![];
                    let pr = self.pull(mr, &path, me, &mut notes).await?;
                    progress.item_completed();
                    Ok::<_, Error>((index, pr, notes))
                }
            })
            .buffer_unordered(self.http.limits().max_concurrent_items)
            .try_collect()
            .await?;
        evaluated.sort_by_key(|(index, _, _)| *index);
        for (_, pr, notes) in evaluated {
            data.pull_requests.push(pr);
            data.coverage_notes.extend(notes);
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
impl Gitlab {
    async fn pull(
        &self,
        mr: MergeRequest,
        repository: &str,
        me: &str,
        notes: &mut Vec<String>,
    ) -> Result<PullRequest> {
        let root = format!("projects/{}/merge_requests/{}", mr.project_id, mr.iid);
        let discussions_path = format!("{root}/discussions");
        let commits_path = format!("{root}/commits");
        let (discussions, commits): (Vec<Discussion>, Vec<Commit>) = tokio::try_join!(
            self.http.pages(&discussions_path, &[]),
            self.http.pages(&commits_path, &[]),
        )?;
        let mine = mr.author.username.eq_ignore_ascii_case(me);
        let url = safe_url(&mr.web_url).ok_or_else(Error::upstream)?;
        let mut flat = Thread {
            id: "conversation".into(),
            url: url.clone(),
            resolved: false,
            events: vec![],
        };
        let mut description_at = mr.created_at;
        let mut description_author = mr.author.clone();
        let mut threads = vec![];
        let mut requested = vec![];
        let mut reviewed = vec![];
        for d in discussions {
            let mut thread = Thread {
                id: d.id,
                url: url.clone(),
                resolved: false,
                events: vec![],
            };
            let resolvable = d.notes.iter().any(|n| n.resolvable);
            thread.resolved =
                resolvable && d.notes.iter().filter(|n| n.resolvable).all(|n| n.resolved);
            for n in d.notes {
                if n.system {
                    if n.body.starts_with("changed the description")
                        && n.created_at >= description_at
                    {
                        description_at = n.created_at;
                        description_author = n.author.clone();
                    }
                    if n.body.contains("requested review from") && attention::mentions(&n.body, me)
                    {
                        requested.push(Event {
                            at: n.created_at,
                            own: false,
                            bot: n.author.bot(),
                            opens: Some(Reason::ReviewRequest),
                            excerpt: n.body.clone(),
                        });
                    }
                    if n.author.username.eq_ignore_ascii_case(me)
                        && (n.body == "approved this merge request"
                            || n.body.starts_with("submitted review"))
                    {
                        reviewed.push(Event {
                            at: n.created_at,
                            own: true,
                            bot: n.author.bot(),
                            opens: None,
                            excerpt: String::new(),
                        });
                    }
                    continue;
                }
                let event = Event {
                    at: n.created_at,
                    own: n.author.username.eq_ignore_ascii_case(me),
                    bot: n.author.bot(),
                    opens: if attention::mentions(&n.body, me) {
                        Some(Reason::PrMention)
                    } else if mine {
                        Some(Reason::AuthorFeedback)
                    } else {
                        None
                    },
                    excerpt: n.body,
                };
                if d.individual_note {
                    flat.events.push(event);
                } else {
                    thread.url = format!("{url}#note_{}", n.id);
                    thread.events.push(event);
                }
            }
            if !thread.events.is_empty() {
                threads.push(thread);
            }
        }
        if mr
            .reviewers
            .iter()
            .any(|u| u.username.eq_ignore_ascii_case(me))
        {
            // Re-request starts a new stream; older requests are not still unanswered.
            if let Some(latest) = requested.into_iter().max_by_key(|e| e.at) {
                let mut events = vec![latest];
                events.extend(reviewed);
                threads.push(Thread {
                    id: "review-request".into(),
                    url: url.clone(),
                    resolved: false,
                    events,
                });
            } else {
                notes.push(format!("Review requested on {url}, but no timestamped request note is accessible. Inspect the MR directly; no timer was fabricated."));
            }
        }
        let description = mr.description.unwrap_or_default();
        if !description_author.username.eq_ignore_ascii_case(me)
            && attention::mentions(&description, me)
        {
            flat.events.push(Event {
                at: description_at,
                own: false,
                bot: description_author.bot(),
                opens: Some(Reason::PrMention),
                excerpt: description.clone(),
            });
        }
        threads.push(flat);
        let (owner, repo) = repository.rsplit_once('/').ok_or_else(Error::upstream)?;
        Ok(PullRequest {
            id: format!("{}:{repository}:{}", self.config.id, mr.iid),
            source_id: self.config.id.clone(),
            forge: Kind::Gitlab,
            number: mr.iid,
            owner: owner.into(),
            repository: repo.into(),
            title: mr.title.clone(),
            url,
            description: description.clone(),
            state: if mr.draft || mr.work_in_progress {
                "draft"
            } else {
                "ready"
            }
            .into(),
            author: mr.author.person(),
            assignees: people(mr.assignees.iter().map(User::person).collect(), me),
            reviewers: people(mr.reviewers.iter().map(User::person).collect(), me),
            labels: mr.labels,
            created_at: mr.created_at,
            updated_at: mr.updated_at,
            last_commit_at: commits.into_iter().filter_map(|c| c.committed_date).max(),
            linked_issue_keys: references(&format!("{}\n{description}", mr.title)),
            is_mine: mine,
            attention: attention::compute(threads, true),
        })
    }
}
