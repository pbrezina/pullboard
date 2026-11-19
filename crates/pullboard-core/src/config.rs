//! The single YAML file is the only application configuration source.
use crate::model::{Kind, Purpose, Workflow};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

/// Validated Rust regex search, compiled once when the configuration is parsed.
#[derive(Debug, Clone)]
pub struct SprintPattern(regex::Regex);

impl SprintPattern {
    pub const MAX_BYTES: usize = 4096;
    pub const COMPILED_SIZE_LIMIT: usize = 1024 * 1024;

    pub fn new(expression: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !expression.trim().is_empty(),
            "sprint pattern must be nonblank"
        );
        anyhow::ensure!(
            expression.len() <= Self::MAX_BYTES,
            "sprint pattern exceeds 4096 UTF-8 bytes"
        );
        regex::RegexBuilder::new(expression)
            .size_limit(Self::COMPILED_SIZE_LIMIT)
            .dfa_size_limit(Self::COMPILED_SIZE_LIMIT)
            .build()
            .map(Self)
            .map_err(|_| anyhow::anyhow!("invalid or oversized compiled sprint regex"))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn is_match(&self, name: &str) -> bool {
        self.0.is_match(name)
    }
}

impl std::str::FromStr for SprintPattern {
    type Err = anyhow::Error;

    fn from_str(expression: &str) -> Result<Self, Self::Err> {
        Self::new(expression)
    }
}

impl<'de> Deserialize<'de> for SprintPattern {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PatternVisitor;
        impl serde::de::Visitor<'_> for PatternVisitor {
            type Value = SprintPattern;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded sprint regex string")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                SprintPattern::new(value).map_err(E::custom)
            }
        }
        // YAML's deserialize_string coerces numeric/bool scalars to strings.
        // The policy contract requires actual regex strings, not coercion.
        deserializer.deserialize_any(PatternVisitor)
    }
}

impl Serialize for SprintPattern {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Secret {
    Literal { value: String },
    Keyring { keyring: KeyringRef },
}
#[derive(Clone, Serialize, Deserialize)]
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
            #[cfg(feature = "native-keyring")]
            Self::Keyring { keyring } => {
                let r = keyring.clone();
                tokio::task::spawn_blocking(move || {
                    let entry = keyring::Entry::new(&r.service, &r.account).map_err(|_| anyhow::anyhow!("keyring entry could not be opened"))?;
                    let value=entry.get_password().map(Zeroizing::new).map_err(|_| anyhow::anyhow!("keyring credential unavailable; unlock the native keyring or check the restricted Secret Service bridge"))?;
                    anyhow::ensure!(!value.trim().is_empty(),"keyring credential is empty");
                    Ok(value)
                }).await.map_err(|_| anyhow::anyhow!("keyring worker failed"))?
            }
            #[cfg(not(feature = "native-keyring"))]
            Self::Keyring { .. } => anyhow::bail!(
                "native keyring resolution is not enabled in this application; reenter the credential explicitly"
            ),
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

pub const DEFAULT_ATTENTION_RETENTION_DAYS: u32 = 180;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub read_only_test_mode: bool,
    pub refresh_interval_seconds: u64,
    pub attention_default_days: u32,
    pub attention_retention_days: u32,
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
            attention_default_days: 14,
            attention_retention_days: DEFAULT_ATTENTION_RETENTION_DAYS,
            upstream: Upstream::default(),
            jira: None,
            forges: vec![],
            ai: AiConfig::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
impl Upstream {
    /// Validate all HTTP, pagination and hydration bounds without a full config.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=120).contains(&self.timeout_seconds)
                && (1..=1000).contains(&self.max_pages)
                && (1..=100).contains(&self.page_size)
                && (1024..=32 * 1024 * 1024).contains(&self.max_response_bytes)
                && (1..=16).contains(&self.max_concurrent_requests)
                && (1..=32).contains(&self.max_concurrent_items)
                && (1..=10).contains(&self.github_batch_size),
            "invalid upstream bounds"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub kind: Kind,
    #[serde(
        default,
        deserialize_with = "explicit_forge_url",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_url: Option<String>,
    pub token: Secret,
    #[serde(default)]
    pub include_repositories: Vec<String>,
    #[serde(default)]
    pub exclude_repositories: Vec<String>,
    #[serde(default)]
    pub default_project: Option<String>,
}

fn explicit_forge_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl ForgeConfig {
    pub fn default_base_url(kind: Kind) -> Option<&'static str> {
        match kind {
            Kind::Github => Some("https://api.github.com"),
            Kind::Gitlab => Some("https://gitlab.com"),
            Kind::Codeberg => Some("https://codeberg.org"),
            Kind::Forgejo | Kind::Jira => None,
        }
    }

    /// The validated, trusted endpoint used by transport, URL routing and cache scope.
    pub fn effective_base_url(&self) -> anyhow::Result<&str> {
        anyhow::ensure!(self.kind != Kind::Jira, "Jira is not a forge");
        let url = self
            .base_url
            .as_deref()
            .or_else(|| Self::default_base_url(self.kind))
            .ok_or_else(|| anyhow::anyhow!("forgejo requires an explicit base_url"))?;
        anyhow::ensure!(base_url(url), "invalid forge base_url");
        Ok(url)
    }

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JiraConfig {
    pub base_url: String,
    pub email: String,
    pub token: Secret,
    pub projects: Vec<String>,
    #[serde(deserialize_with = "board_ids")]
    pub board_ids: Vec<u64>,
    pub team: String,
    #[serde(deserialize_with = "sprint_name_patterns")]
    pub sprint_name_patterns: Vec<SprintPattern>,
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
fn board_ids<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    let values = Vec::<u64>::deserialize(d)?;
    validate_board_ids(&values).map_err(serde::de::Error::custom)?;
    Ok(values)
}
fn sprint_name_patterns<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SprintPattern>, D::Error> {
    let values = Vec::<SprintPattern>::deserialize(d)?;
    validate_sprint_patterns(&values).map_err(serde::de::Error::custom)?;
    Ok(values)
}
/// Validate an ordered selection without sorting or silently removing duplicates.
pub fn validate_board_ids(values: &[u64]) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    anyhow::ensure!(
        (1..=100).contains(&values.len()) && values.iter().all(|id| *id > 0 && seen.insert(id)),
        "Jira requires 1..100 unique positive board IDs"
    );
    Ok(())
}
/// Patterns are already bounded and compiled; validation never recompiles them.
pub fn validate_sprint_patterns(values: &[SprintPattern]) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    anyhow::ensure!(
        (1..=100).contains(&values.len()) && values.iter().all(|p| seen.insert(p.as_str())),
        "Jira requires 1..100 unique sprint patterns"
    );
    Ok(())
}
fn group_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Vec::<String>::deserialize(d)?
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurposeDefaults {
    #[serde(
        default,
        deserialize_with = "optional_object",
        skip_serializing_if = "Option::is_none"
    )]
    pub implementation: Option<PurposePolicy>,
    #[serde(
        default,
        deserialize_with = "optional_object",
        skip_serializing_if = "Option::is_none"
    )]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Only the primary board and primary pattern can identify a current sprint.
    pub fn current_sprint(&self, board: u64, state: &str, name: &str) -> bool {
        state == "active"
            && self.board_ids.first() == Some(&board)
            && self
                .sprint_name_patterns
                .first()
                .is_some_and(|p| p.is_match(name))
    }
    pub fn allows_sprint(&self, name: &str) -> bool {
        self.sprint_name_patterns.iter().any(|p| p.is_match(name))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub enabled: bool,
    pub provider: AiProvider,
    pub model: String,
    pub base_url: Option<String>,
    pub token: Option<Secret>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
    pub const MAX_YAML_BYTES: usize = 1024 * 1024;

    /// Parse and validate the same schema used by the backend, without resolving secrets.
    /// Errors deliberately omit YAML values, which can contain credentials.
    pub fn from_yaml(input: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            input.len() <= Self::MAX_YAML_BYTES,
            "configuration file exceeds the one MiB limit"
        );
        let config: Self = serde_yaml::from_str(input).map_err(|_| anyhow::anyhow!("invalid YAML configuration (unknown fields, ambiguous secrets and environment references are not supported)"))?;
        config.validate()?;
        Ok(config)
    }

    /// Serialize validated settings, including secret sources, for a protected YAML file.
    /// The returned buffer is secret: never display it in summaries or logs.
    /// Defaults can become explicit; YAML comments are not retained.
    pub fn to_yaml(&self) -> anyhow::Result<Zeroizing<String>> {
        self.validate()?;
        let output = Zeroizing::new(
            serde_yaml::to_string(self)
                .map_err(|_| anyhow::anyhow!("configuration could not be serialized"))?,
        );
        anyhow::ensure!(
            output.len() <= Self::MAX_YAML_BYTES,
            "configuration file exceeds the one MiB limit"
        );
        Ok(output)
    }

    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        use tokio::io::AsyncReadExt;
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| anyhow::anyhow!("configuration file could not be read"))?;
        let mut input = Zeroizing::new(String::new());
        file.take((Self::MAX_YAML_BYTES + 1) as u64)
            .read_to_string(&mut input)
            .await
            .map_err(|_| anyhow::anyhow!("configuration file could not be read"))?;
        Self::from_yaml(&input)
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
            (1..=36500).contains(&self.attention_default_days),
            "attention_default_days must be a whole number from 1 to 36500"
        );
        anyhow::ensure!(
            (1..=36500).contains(&self.attention_retention_days),
            "attention_retention_days must be a whole number from 1 to 36500"
        );
        anyhow::ensure!(
            self.forges.len() <= 64,
            "at most 64 forge instances can be configured"
        );
        self.upstream.validate()?;
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
            f.effective_base_url()?;
            anyhow::ensure!(
                !f.name().trim().is_empty() && f.token.validate(),
                "invalid forge account or credential"
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
            anyhow::ensure!(!j.team.trim().is_empty(), "Jira team is required");
            validate_board_ids(&j.board_ids)?;
            validate_sprint_patterns(&j.sprint_name_patterns)?;
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
