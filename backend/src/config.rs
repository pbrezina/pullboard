//! The single YAML file is the only application configuration source.
use crate::model::{Kind, Purpose, Workflow};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Secret {
    Literal { value: String },
    Keyring { keyring: KeyringRef },
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyringRef {
    pub service: String,
    pub account: String,
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
impl Secret {
    pub async fn resolve(&self) -> anyhow::Result<Zeroizing<String>> {
        match self {
            Self::Literal { value } => Ok(Zeroizing::new(value.clone())),
            Self::Keyring { keyring } => {
                let r = keyring.clone();
                tokio::task::spawn_blocking(move || {
                    let entry = keyring::Entry::new(&r.service, &r.account).map_err(|_| anyhow::anyhow!("keyring entry could not be opened"))?;
                    let value=entry.get_password().map(Zeroizing::new).map_err(|_| anyhow::anyhow!("keyring credential unavailable; unlock the native keyring or check the restricted Secret Service bridge"))?;
                    anyhow::ensure!(!value.trim().is_empty(),"keyring credential is empty");
                    Ok(value)
                }).await.map_err(|_| anyhow::anyhow!("keyring worker failed"))?
            }
        }
    }
    fn validate(&self) -> bool {
        match self {
            Self::Literal { value } => !value.trim().is_empty(),
            Self::Keyring { keyring } => {
                !keyring.service.trim().is_empty() && !keyring.account.trim().is_empty()
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub read_only_test_mode: bool,
    pub refresh_interval_seconds: u64,
    pub upstream: Upstream,
    pub jira: Option<JiraConfig>,
    pub forges: Vec<ForgeConfig>,
    pub ai: AiConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            server: Server::default(),
            read_only_test_mode: true,
            refresh_interval_seconds: 3600,
            upstream: Upstream::default(),
            jira: None,
            forges: vec![],
            ai: AiConfig::default(),
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub bind: IpAddr,
    pub port: u16,
    pub allowed_origins: Vec<String>,
    pub static_directory: PathBuf,
    pub database: PathBuf,
}
impl Default for Server {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8080,
            allowed_origins: vec![
                "http://localhost:8080".into(),
                "http://127.0.0.1:8080".into(),
            ],
            static_directory: "frontend/dist/frontend/browser".into(),
            database: "pullboard-cache.sqlite".into(),
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Upstream {
    pub timeout_seconds: u64,
    pub max_pages: u32,
    pub page_size: u32,
    pub max_response_bytes: usize,
    pub max_concurrent_requests: usize,
    pub max_concurrent_items: usize,
    pub github_batch_size: usize,
}
impl Default for Upstream {
    fn default() -> Self {
        Self {
            timeout_seconds: 30,
            max_pages: 100,
            page_size: 100,
            max_response_bytes: 8 * 1024 * 1024,
            max_concurrent_requests: 4,
            max_concurrent_items: 4,
            github_batch_size: 5,
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub kind: Kind,
    pub base_url: String,
    pub token: Secret,
    #[serde(default)]
    pub include_repositories: Vec<String>,
    #[serde(default)]
    pub exclude_repositories: Vec<String>,
    #[serde(default)]
    pub default_project: Option<String>,
}
impl ForgeConfig {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
    pub fn includes(&self, repository: &str) -> bool {
        let matches = |patterns: &[String]| {
            patterns.iter().any(|p| {
                globset::Glob::new(p).is_ok_and(|g| g.compile_matcher().is_match(repository))
            })
        };
        (self.include_repositories.is_empty() || matches(&self.include_repositories))
            && !matches(&self.exclude_repositories)
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JiraConfig {
    pub base_url: String,
    pub email: String,
    pub token: Secret,
    pub projects: Vec<String>,
    pub board_id: u64,
    pub team: String,
    pub sprint_name_pattern: String,
    #[serde(default = "team_field_name")]
    pub team_field_name: String,
    #[serde(default, deserialize_with = "group_names")]
    pub assignee_groups: Vec<String>,
    #[serde(default)]
    pub defaults: PurposeDefaults,
    #[serde(default)]
    pub project_overrides: std::collections::BTreeMap<String, ProjectOverride>,
}
fn team_field_name() -> String {
    "AssignedTeam".into()
}
fn group_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Vec::<String>::deserialize(d)?
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurposeDefaults {
    #[serde(default, deserialize_with = "optional_object")]
    pub implementation: Option<PurposePolicy>,
    #[serde(default, deserialize_with = "optional_object")]
    pub review: Option<PurposePolicy>,
}
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurposePolicy {
    pub issue_type: String,
    #[serde(deserialize_with = "required_nullable")]
    pub assignee: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow: Option<Workflow>,
}
fn optional_object<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
fn required_nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectOverride {
    #[serde(default)]
    pub defaults: PurposeDefaults,
}
impl JiraConfig {
    pub fn policy(&self, key: &str, purpose: Purpose) -> PurposePolicy {
        let pick = |d: &PurposeDefaults| match purpose {
            Purpose::Implementation => d.implementation.clone(),
            Purpose::Review => d.review.clone(),
        };
        let mut policy = pick(&self.defaults).unwrap_or(PurposePolicy {
            issue_type: "Task".into(),
            assignee: None,
            workflow: None,
        });
        if let Some(mut override_policy) = self
            .project_overrides
            .get(key)
            .and_then(|o| pick(&o.defaults))
        {
            if override_policy.workflow.is_none() {
                override_policy.workflow = policy.workflow;
            }
            policy = override_policy;
        }
        policy
    }
    pub fn sprint_pattern(&self) -> anyhow::Result<regex::Regex> {
        anyhow::ensure!(
            !self.sprint_name_pattern.trim().is_empty(),
            "sprint pattern must be nonblank"
        );
        let mut input = self.sprint_name_pattern.as_str();
        let mut expression = String::from(r"\A");
        while let Some(at) = input.find('%') {
            expression.push_str(&regex::escape(&input[..at]));
            input = &input[at..];
            if let Some(rest) = input.strip_prefix("%YEAR") {
                expression.push_str("[0-9]{4}");
                input = rest;
            } else if let Some(rest) = input.strip_prefix("%NUMBER") {
                expression.push_str("[0-9]+");
                input = rest;
            } else {
                anyhow::bail!("unsupported sprint pattern token");
            }
            anyhow::ensure!(
                !input.starts_with(|c: char| c.is_ascii_alphabetic()),
                "malformed sprint pattern token"
            );
        }
        expression.push_str(&regex::escape(input));
        expression.push_str(r"\z");
        regex::Regex::new(&expression).map_err(|_| anyhow::anyhow!("invalid sprint pattern"))
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub enabled: bool,
    pub provider: AiProvider,
    pub model: String,
    pub base_url: Option<String>,
    pub token: Option<Secret>,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProvider {
    Openai,
    Anthropic,
    Ollama,
}
impl AiProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
        }
    }
}
impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: AiProvider::Openai,
            model: "gpt-6-luna".into(),
            base_url: None,
            token: None,
        }
    }
}

pub fn http_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
pub fn safe_url(value: &str) -> Option<String> {
    http_url(value).then(|| value.to_owned())
}
fn base_url(value: &str) -> bool {
    http_url(value)
        && url::Url::parse(value).is_ok_and(|u| u.query().is_none() && u.fragment().is_none())
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
pub fn project_key(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}
impl Config {
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|_| anyhow::anyhow!("configuration file could not be read"))?;
        anyhow::ensure!(
            metadata.len() <= 1024 * 1024,
            "configuration file exceeds the one MiB limit"
        );
        let input = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| anyhow::anyhow!("configuration file could not be read"))?;
        let config: Self = serde_yaml::from_str(&input).map_err(|_| anyhow::anyhow!("invalid YAML configuration (unknown fields, ambiguous secrets and environment references are not supported)"))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.server.bind.is_loopback() || self.server.bind.is_unspecified(),
            "bind must be loopback or an explicitly container-published unspecified address"
        );
        anyhow::ensure!(
            self.server.port >= 1024 && (1..=604800).contains(&self.refresh_interval_seconds),
            "port must be unprivileged (1024..65535) and refresh interval must be 1..604800 seconds"
        );
        anyhow::ensure!(
            self.forges.len() <= 64,
            "at most 64 forge instances can be configured"
        );
        anyhow::ensure!(
            (1..=120).contains(&self.upstream.timeout_seconds)
                && (1..=1000).contains(&self.upstream.max_pages)
                && (1..=100).contains(&self.upstream.page_size)
                && (1024..=32 * 1024 * 1024).contains(&self.upstream.max_response_bytes)
                && (1..=16).contains(&self.upstream.max_concurrent_requests)
                && (1..=32).contains(&self.upstream.max_concurrent_items)
                && (1..=10).contains(&self.upstream.github_batch_size),
            "invalid upstream bounds"
        );
        for origin in &self.server.allowed_origins {
            let u = url::Url::parse(origin)
                .map_err(|_| anyhow::anyhow!("invalid allowed local origin"))?;
            let local = u.host_str().is_some_and(|h| {
                h == "localhost"
                    || h.trim_matches(['[', ']'])
                        .parse::<IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            anyhow::ensure!(
                http_url(origin)
                    && local
                    && u.path() == "/"
                    && u.query().is_none()
                    && u.fragment().is_none()
                    && !origin.ends_with('/'),
                "allowed origins must be exact HTTP(S) loopback origins without a trailing slash"
            );
        }
        let mut ids = BTreeSet::from(["jira".to_owned()]);
        for f in &self.forges {
            anyhow::ensure!(
                identifier(&f.id) && ids.insert(f.id.clone()) && f.kind != Kind::Jira,
                "forge IDs must be unique, stable and not jira"
            );
            anyhow::ensure!(
                base_url(&f.base_url) && !f.name().trim().is_empty() && f.token.validate(),
                "invalid forge URL, account or credential"
            );
            for p in f.include_repositories.iter().chain(&f.exclude_repositories) {
                anyhow::ensure!(globset::Glob::new(p).is_ok(), "invalid repository glob");
            }
        }
        if let Some(j) = &self.jira {
            anyhow::ensure!(
                !j.projects.is_empty() && j.projects.len() <= 100,
                "Jira requires 1..100 selected projects"
            );
            anyhow::ensure!(
                base_url(&j.base_url) && !j.email.trim().is_empty() && j.token.validate(),
                "invalid Jira URL, account or credential"
            );
            let mut projects = BTreeSet::new();
            for p in &j.projects {
                anyhow::ensure!(
                    project_key(p) && projects.insert(p),
                    "invalid or duplicate Jira project"
                );
            }
            anyhow::ensure!(
                j.board_id > 0
                    && !j.team.trim().is_empty()
                    && !j.sprint_name_pattern.trim().is_empty(),
                "Jira board, team and sprint pattern are required"
            );
            anyhow::ensure!(
                !j.team_field_name.trim().is_empty()
                    && !j.team_field_name.to_lowercase().starts_with("customfield")
                    && !j.team_field_name.bytes().all(|b| b.is_ascii_digit())
                    && !j
                        .team_field_name
                        .strip_prefix("field")
                        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())),
                "team field selector must be a semantic name"
            );
            j.sprint_pattern()?;
            anyhow::ensure!(
                j.assignee_groups.iter().all(|g| !g.trim().is_empty()),
                "group names must be nonblank"
            );
            for (key, o) in &j.project_overrides {
                anyhow::ensure!(
                    projects.contains(key),
                    "project override must select a configured project"
                );
                validate_defaults(&o.defaults)?;
            }
            validate_defaults(&j.defaults)?;
            for f in &self.forges {
                anyhow::ensure!(
                    f.default_project
                        .as_ref()
                        .is_none_or(|k| projects.contains(k)),
                    "forge default project must be configured"
                );
            }
        } else {
            anyhow::ensure!(
                self.forges.iter().all(|f| f.default_project.is_none()),
                "forge default project requires Jira"
            );
        }
        anyhow::ensure!(
            !self.ai.model.trim().is_empty()
                && self.ai.base_url.as_ref().is_none_or(|u| base_url(u))
                && self.ai.token.as_ref().is_none_or(Secret::validate),
            "invalid AI configuration"
        );
        anyhow::ensure!(
            !self.ai.enabled
                || matches!(self.ai.provider, AiProvider::Ollama)
                || self.ai.token.is_some(),
            "enabled cloud AI requires a YAML or keyring token"
        );
        Ok(())
    }
    pub async fn check_secrets(&self) -> anyhow::Result<()> {
        for f in &self.forges {
            let _ = f.token.resolve().await?;
        }
        if let Some(j) = &self.jira {
            let _ = j.token.resolve().await?;
        }
        if self.ai.enabled
            && let Some(s) = &self.ai.token
        {
            let _ = s.resolve().await?;
        }
        Ok(())
    }
}

fn validate_defaults(d: &PurposeDefaults) -> anyhow::Result<()> {
    for (p, review) in [(&d.implementation, false), (&d.review, true)] {
        if let Some(p) = p {
            anyhow::ensure!(
                !p.issue_type.trim().is_empty()
                    && p.assignee.as_ref().is_none_or(|a| !review && a == "me"),
                "invalid purpose defaults"
            );
            if let Some(w) = &p.workflow {
                anyhow::ensure!(
                    [&w.draft, &w.ready]
                        .into_iter()
                        .flatten()
                        .all(|s| !s.trim().is_empty()),
                    "workflow suggestions must be nonblank or null"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_defaults() {
        Config::default().validate().unwrap();
        assert_eq!(Config::default().ai.model, "gpt-6-luna");
    }
    #[test]
    fn secrets_reject_ambiguity_and_env() {
        assert!(
            serde_yaml::from_str::<Secret>("value: abc\nkeyring: {service: x, account: y}")
                .is_err()
        );
        assert!(serde_yaml::from_str::<Secret>("env: TOKEN").is_err());
        let s: Secret = serde_yaml::from_str("value: top-secret").unwrap();
        assert!(!format!("{s:?}").contains("top-secret"));
    }
    #[test]
    fn reject_unknown_config_and_bad_origins() {
        assert!(serde_yaml::from_str::<Config>("password: wrong").is_err());
        let mut c = Config::default();
        c.server.allowed_origins = vec!["https://evil.example".into()];
        assert!(c.validate().is_err());
    }
    #[test]
    fn urls_disallow_scripts_and_credentials() {
        assert!(!http_url("javascript:alert(1)"));
        assert!(!http_url("https://token@host"));
        assert!(http_url("https://host/x"));
    }
}
