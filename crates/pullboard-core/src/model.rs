//! Types shared by YAML policy and backend wire models (never duplicate these).
use serde::{Deserialize, Serialize};

pub type Time = chrono::DateTime<chrono::Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Jira,
    Github,
    Gitlab,
    Codeberg,
    Forgejo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Implementation,
    Review,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    #[serde(deserialize_with = "nullable_status")]
    pub draft: Option<String>,
    #[serde(deserialize_with = "nullable_status")]
    pub ready: Option<String>,
}

fn nullable_status<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
