//! Forge adapters expose discovery only. No write method exists in this interface.
mod forgejo;
mod github;
mod gitlab;
pub(crate) use github::segment;

use crate::{
    cache::SourceData,
    config::{ForgeConfig, Upstream},
    error::Result,
    model::{Identity, Kind, Person, UpstreamIssue},
    sync::Progress,
    transport::ReadTransport,
};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait Forge: Send + Sync {
    fn source_id(&self) -> &str;
    async fn identity(&self) -> Result<Identity>;
    async fn discover(&self) -> Result<SourceData> {
        self.discover_with_progress(Progress::default()).await
    }
    async fn discover_with_progress(&self, progress: Progress) -> Result<SourceData>;
    async fn issue(
        &self,
        _route: &crate::upstream::IssueRoute,
        _identity: &Identity,
    ) -> Result<UpstreamIssue> {
        Err(crate::error::Error::upstream())
    }
}
pub async fn build(config: &ForgeConfig, limits: &Upstream) -> anyhow::Result<Arc<dyn Forge>> {
    let transport = ReadTransport::new(config, limits).await?;
    Ok(match config.kind {
        Kind::Github => Arc::new(github::Github::new(config.clone(), transport)),
        Kind::Gitlab => Arc::new(gitlab::Gitlab::new(config.clone(), transport)),
        Kind::Codeberg | Kind::Forgejo => {
            Arc::new(forgejo::Forgejo::new(config.clone(), transport))
        }
        Kind::Jira => anyhow::bail!("Jira is not a forge"),
    })
}
pub fn people(mut people: Vec<Person>, me: &str) -> Vec<Person> {
    people.retain(|p| !p.username.eq_ignore_ascii_case(me));
    let mut seen = std::collections::BTreeSet::new();
    people.retain(|p| seen.insert(p.id.clone()));
    people.sort_by(|a, b| {
        a.username
            .to_lowercase()
            .cmp(&b.username.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    people
}
pub fn references(text: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"\b[A-Z][A-Z0-9_]+-[1-9][0-9]*\b").expect("constant regex")
    });
    re.find_iter(text)
        .map(|m| m.as_str().to_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
