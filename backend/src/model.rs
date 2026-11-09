//! Wire types match api/openapi.json; nullable fields are never omitted.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Time = DateTime<Utc>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Jira,
    Github,
    Gitlab,
    Codeberg,
    Forgejo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    JiraMention,
    ReviewRequest,
    PrMention,
    AuthorFeedback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub url: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraAssignee {
    #[serde(flatten)]
    pub person: Person,
    pub email: Option<String>,
}
impl std::ops::Deref for JiraAssignee {
    type Target = Person;
    fn deref(&self) -> &Person {
        &self.person
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub source_id: String,
    pub kind: Kind,
    pub account_id: String,
    pub display_name: String,
    pub username: String,
    pub profile_url: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sprint {
    pub id: String,
    pub board_id: u64,
    pub team: Option<String>,
    pub default_eligible: bool,
    pub name: String,
    pub state: String,
    pub board_url: String,
    pub start_at: Option<Time>,
    pub end_at: Option<Time>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionThread {
    pub id: String,
    pub url: String,
    pub reason: Reason,
    pub waiting_since: Time,
    pub excerpt: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attention {
    pub reasons: Vec<Reason>,
    pub waiting_since: Time,
    pub threads: Vec<AttentionThread>,
    pub coverage: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssue {
    pub id: String,
    pub source_id: String,
    pub key: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub project_key: String,
    pub issue_type: String,
    pub status: String,
    pub reporter: Option<Person>,
    pub assignees: Vec<Person>,
    pub is_assigned_to_me: bool,
    pub created_at: Time,
    pub updated_at: Time,
    pub sprint_ids: Vec<String>,
    pub linked_pr_ids: Vec<String>,
    pub attention: Option<Attention>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: String,
    pub source_id: String,
    pub forge: Kind,
    pub number: u64,
    pub owner: String,
    pub repository: String,
    pub title: String,
    pub url: String,
    pub description: String,
    pub state: String,
    pub author: Person,
    pub assignees: Vec<Person>,
    pub reviewers: Vec<Person>,
    pub labels: Vec<String>,
    pub created_at: Time,
    pub updated_at: Time,
    pub last_commit_at: Option<Time>,
    pub linked_issue_keys: Vec<String>,
    pub is_mine: bool,
    pub attention: Option<Attention>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamIssue {
    pub id: String,
    pub source_id: String,
    pub forge: Kind,
    pub number: u64,
    pub owner: String,
    pub repository: String,
    pub title: String,
    pub url: String,
    pub description: String,
    pub state: String,
    pub reporter: Option<Person>,
    pub assignees: Vec<Person>,
    pub labels: Vec<String>,
    pub is_assigned_to_me: bool,
    pub created_at: Time,
    pub updated_at: Time,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveUpstreamIssueRequest {
    pub source_id: Option<String>,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedUpstreamIssue {
    pub issue: UpstreamIssue,
    pub warning: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamIssueDraftRequest {
    pub upstream_issue_id: String,
    pub project_key: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamIssueDraft {
    pub upstream_issue_id: String,
    pub purpose: ImplementationPurpose,
    pub project_key: String,
    pub title: String,
    pub description: String,
    pub story_points: Option<f64>,
    pub assignee_id: Option<String>,
    pub sprint_id: Option<String>,
    pub issue_type: Option<String>,
    pub team: Option<String>,
    pub suggested_status: Option<String>,
    pub generated_by: String,
    pub warning: Option<String>,
    pub diagnostics: Vec<JiraDiagnostic>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationPurpose {
    Implementation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUpstreamIssueRequest {
    pub upstream_issue_id: String,
    pub expected_mode_revision: String,
    pub purpose: ImplementationPurpose,
    pub project_key: String,
    pub issue_type: String,
    pub title: String,
    pub description: String,
    pub story_points: Option<f64>,
    pub assignee_id: Option<String>,
    pub sprint_id: Option<String>,
    pub team: Option<String>,
    pub status: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceHealth {
    pub source_id: String,
    pub name: String,
    pub kind: Kind,
    pub status: String,
    pub last_success_at: Option<Time>,
    pub last_attempt_at: Option<Time>,
    pub error: Option<String>,
    pub retry_at: Option<Time>,
    pub coverage_notes: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Board {
    pub revision: u64,
    pub generated_at: Option<Time>,
    pub issues: Vec<JiraIssue>,
    pub pull_requests: Vec<PullRequest>,
    #[serde(default)]
    pub upstream_issues: Vec<UpstreamIssue>,
    pub sprints: Vec<Sprint>,
    pub sources: Vec<SourceHealth>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub read_only_test_mode: bool,
    pub mode_revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMode {
    pub read_only_test_mode: bool,
    pub mode_revision: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRuntimeModeRequest {
    pub read_only_test_mode: bool,
    pub expected_mode_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub key: String,
    pub name: String,
    pub issue_types: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiContext {
    pub enabled: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub read_only_test_mode: bool,
    pub mode_revision: String,
    pub refresh_interval_seconds: u64,
    pub identities: Vec<Identity>,
    pub current_project: Option<Project>,
    pub current_sprints: Vec<Sprint>,
    pub jira_projects: Vec<Project>,
    pub ai: AiContext,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub state: String,
    pub started_at: Option<Time>,
    pub finished_at: Option<Time>,
    pub last_success_at: Option<Time>,
    pub sources: Vec<SourceHealth>,
    pub progress: Vec<SourceProgress>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePhase {
    Idle,
    Queued,
    Preparing,
    Discovering,
    Fetching,
    Publishing,
    Completed,
    Failed,
    Backoff,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceProgress {
    pub source_id: String,
    pub phase: SourcePhase,
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub started_at: Option<Time>,
    pub finished_at: Option<Time>,
    pub elapsed_ms: u64,
}
impl SourceProgress {
    pub fn new(source_id: String, phase: SourcePhase) -> Self {
        Self {
            source_id,
            phase,
            completed_items: 0,
            total_items: None,
            started_at: None,
            finished_at: None,
            elapsed_ms: 0,
        }
    }
}
impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            started_at: None,
            finished_at: None,
            last_success_at: None,
            sources: vec![],
            progress: vec![],
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraDefaults {
    pub issue_type: Option<String>,
    pub assignee_id: Option<String>,
    pub team: Option<String>,
    pub sprint_id: Option<String>,
    pub workflow: Workflow,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraOptions {
    pub project_key: String,
    pub purpose: Purpose,
    pub issue_type: Option<String>,
    pub assignees: Vec<JiraAssignee>,
    pub sprints: Vec<Sprint>,
    pub issue_types: Vec<String>,
    pub defaults: JiraDefaults,
    pub teams: Vec<TeamChoice>,
    pub statuses: Vec<StatusChoice>,
    pub capabilities: CreateCapabilities,
    pub diagnostics: Vec<JiraDiagnostic>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamChoice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusChoice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraDiagnostic {
    pub code: String,
    pub field: Option<String>,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldCapability {
    pub mode: String,
    pub required: bool,
    pub diagnostic: Option<JiraDiagnostic>,
}
impl FieldCapability {
    pub fn supported(&self) -> bool {
        matches!(self.mode.as_str(), "create" | "follow_up")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCapabilities {
    pub can_create: bool,
    pub unassigned_allowed: bool,
    pub story_points: FieldCapability,
    pub team: FieldCapability,
    pub sprint: FieldCapability,
    pub blockers: Vec<JiraDiagnostic>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Implementation,
    Review,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRequest {
    pub pull_request_id: String,
    pub purpose: Purpose,
    pub project_key: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketDraft {
    pub pull_request_id: String,
    pub purpose: Purpose,
    pub project_key: String,
    pub title: String,
    pub description: String,
    pub story_points: Option<f64>,
    pub assignee_id: Option<String>,
    pub sprint_id: Option<String>,
    pub issue_type: Option<String>,
    pub team: Option<String>,
    pub suggested_status: Option<String>,
    pub generated_by: String,
    pub warning: Option<String>,
    pub diagnostics: Vec<JiraDiagnostic>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueRequest {
    pub pull_request_id: String,
    pub expected_mode_revision: String,
    pub purpose: Purpose,
    pub project_key: String,
    pub issue_type: String,
    pub title: String,
    pub description: String,
    pub story_points: Option<f64>,
    pub assignee_id: Option<String>,
    pub sprint_id: Option<String>,
    pub team: Option<String>,
    pub status: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueReference {
    pub key: String,
    pub url: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIssueResult {
    pub outcome: String,
    pub issue: Option<IssueReference>,
    pub message: String,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    pub pull_request_id: String,
    pub expected_mode_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkResult {
    pub outcome: String,
    pub issue_key: String,
    pub pull_request_id: String,
    pub message: String,
}
