//! Account-scoped Jira Cloud discovery. Only complete successful reads are cached.
use super::*;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

/// Complete private refresh-time choices, stored in the same transaction as cards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub account_id: String,
    pub projects: Vec<Project>,
    pub types: BTreeMap<String, BTreeMap<String, TypeMetadata>>,
    pub sprint_field: Option<String>,
    pub assignees: BTreeMap<String, Vec<JiraAssignee>>,
    pub sprints: Vec<Sprint>,
    assignee_groups: Vec<String>,
    team: String,
    policies: BTreeMap<String, [crate::config::PurposePolicy; 2]>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMetadata {
    pub id: String,
    pub points_field: Option<String>,
    pub sprint_field: Option<String>,
    pub team_field: Option<String>,
    pub team_values: BTreeMap<String, Value>,
    teams: Vec<TeamChoice>,
    capabilities: CreateCapabilities,
    create_fields: BTreeMap<String, Field>,
    statuses: Vec<StatusChoice>,
}
impl TypeMetadata {
    pub(super) fn permits(&self, field: &str) -> bool {
        self.create_fields.get(field).is_some_and(writable)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Field {
    id: String,
    name: String,
    #[serde(default)]
    required: bool,
    #[serde(default, rename = "hasDefaultValue")]
    has_default: bool,
    #[serde(default)]
    schema: Value,
    #[serde(default, rename = "allowedValues")]
    allowed_values: Option<Vec<Value>>,
    #[serde(default)]
    operations: Vec<String>,
}
pub(super) fn failed() -> Error {
    Error::new(
        StatusCode::BAD_GATEWAY,
        "jira_discovery_failed",
        "Jira discovery failed or returned incomplete data; check permissions and pagination bounds.",
    )
}
pub(super) fn discovery_error(error: Error) -> Error {
    if error.retry_at.is_some()
        || error.status == StatusCode::TOO_MANY_REQUESTS
        || error.code == "jira_not_ready"
    {
        error
    } else {
        failed()
    }
}
fn ambiguous() -> Error {
    Error::new(
        StatusCode::BAD_GATEWAY,
        "jira_metadata_ambiguous",
        "Jira metadata is ambiguous; use a unique semantic field and issue type name.",
    )
}
pub(super) fn invalid_choice() -> Error {
    Error::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_choice",
        "A submitted Jira choice is not currently permitted.",
    )
}
pub(super) fn not_ready() -> Error {
    Error::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "jira_not_ready",
        "Jira identity and metadata discovery are not ready; refresh the source first.",
    )
}
fn diagnostic(code: &str, field: &str, message: &str) -> JiraDiagnostic {
    JiraDiagnostic {
        code: code.into(),
        field: Some(field.into()),
        message: message.into(),
    }
}
fn capability(mode: &str, required: bool, field: &str) -> FieldCapability {
    FieldCapability {
        mode: mode.into(),
        required,
        diagnostic: (!matches!(mode, "create" | "follow_up")).then(|| {
            diagnostic(
                &format!("{field}_{mode}"),
                if field == "sprint" {
                    "sprint_id"
                } else {
                    field
                },
                "This field has no supported discovered write mechanism for this issue type.",
            )
        }),
    }
}
fn unique<'a>(fields: impl Iterator<Item = &'a Field>) -> Result<Option<&'a Field>> {
    let mut fields = fields;
    let first = fields.next();
    if fields.next().is_some() {
        return Err(ambiguous());
    }
    Ok(first)
}
fn schema(field: &Field, key: &str, value: &str) -> bool {
    field.schema.get(key).and_then(Value::as_str) == Some(value)
}
fn sprint(field: &Field) -> bool {
    schema(field, "custom", "com.pyxis.greenhopper.jira:gh-sprint")
}
fn writable(field: &Field) -> bool {
    field.operations.iter().any(|op| op == "set")
        // The type-specific create endpoint establishes the issue type's context.
        // Jira can omit setter operations for this selector while still requiring
        // its ID on create. Editable and custom fields must still advertise set.
        || (field.id == "issuetype"
            && field.operations.is_empty()
            && schema(field, "type", "issuetype")
            && schema(field, "system", "issuetype"))
}

impl Jira {
    /// Identity verification is deliberately independent of issue/metadata refresh.
    pub async fn identity(&self) -> Result<Identity> {
        let me: User = self
            .get("rest/api/3/myself", &[])
            .await
            .map_err(discovery_error)?;
        if me.account_id.trim().is_empty() || !me.active || me.bot() {
            return Err(failed());
        }
        let mut principal = self.principal.lock().await;
        if principal
            .as_ref()
            .is_some_and(|old| old.account_id != me.account_id)
        {
            *self.metadata_error.lock().await = None;
            self.conditional.lock().await.clear();
        }
        let identity = Identity {
            source_id: "jira".into(),
            kind: Kind::Jira,
            account_id: me.account_id.clone(),
            display_name: me.display_name.clone(),
            username: me.email.clone().unwrap_or_default(),
            profile_url: None,
        };
        *principal = Some(me);
        drop(principal);
        if let Some(cache) = self.cache.get() {
            let snapshot = cache.read().await?;
            if !snapshot.identities.iter().any(|i| {
                i.source_id == "jira" && i.kind == Kind::Jira && i.account_id == identity.account_id
            }) {
                cache.verify(identity.clone()).await?;
            }
        }
        Ok(identity)
    }
    pub(super) async fn account_id(&self) -> Result<String> {
        self.principal
            .lock()
            .await
            .as_ref()
            .map(|p| p.account_id.clone())
            .ok_or_else(not_ready)
    }
    pub async fn projects(&self) -> Vec<Project> {
        self.choices().await.map(|m| m.projects).unwrap_or_default()
    }
    /// Bind the single transactional store before serving requests or refreshing.
    pub fn bind_cache(&self, cache: Arc<crate::cache::Cache>) {
        self.cache
            .set(cache)
            .unwrap_or_else(|_| panic!("Jira cache already bound"));
    }
    /// Explicit refresh helper for standalone adapter consumers. Production uses Syncer.
    pub async fn refresh_metadata(&self) -> Result<()> {
        let cache = self.cache.get().ok_or_else(not_ready)?;
        let result = self.discover().await;
        let error = result.as_ref().err().cloned();
        if let Err(error) = cache
            .publish(vec![crate::cache::SourceUpdate {
                source_id: "jira".into(),
                result,
            }])
            .await
        {
            self.choice_failure(error.clone()).await;
            return Err(error);
        }
        error.map_or(Ok(()), Err)
    }
    pub(super) async fn choices(&self) -> Result<Metadata> {
        let account = self.account_id().await?;
        let cache = self.cache.get().ok_or_else(not_ready)?;
        let snapshot = cache.read().await?;
        if let Some(choices) = snapshot.jira_choices.filter(|m| m.account_id == account) {
            return Ok(choices);
        }
        if let Some(error) = &*self.metadata_error.lock().await {
            return Err(if error.code == "jira_metadata_ambiguous" {
                error.clone()
            } else {
                discovery_error(error.clone())
            });
        }
        if snapshot
            .board
            .sources
            .iter()
            .any(|s| s.source_id == "jira" && matches!(s.status.as_str(), "error" | "backoff"))
        {
            return Err(failed());
        }
        Err(not_ready())
    }
    /// Jira's current create-metadata endpoints return paginated values. Reject
    /// repeated offsets, missing totals and exhausted bounds instead of partial pools.
    async fn values(&self, path: &str, params: &[(&str, String)]) -> Result<Vec<Value>> {
        let mut start = 0u64;
        let mut out = vec![];
        for _ in 0..self.limits.max_pages {
            let mut params = params.to_vec();
            params.extend([
                ("startAt", start.to_string()),
                ("maxResults", self.limits.page_size.to_string()),
            ]);
            let page: Value = self.get(path, &params).await.map_err(discovery_error)?;
            let values = page
                .get("values")
                .or_else(|| page.get("issueTypes"))
                .or_else(|| page.get("fields"))
                .and_then(Value::as_array)
                .ok_or_else(failed)?;
            if page.get("startAt").and_then(Value::as_u64) != Some(start) {
                return Err(failed());
            }
            let count = u64::try_from(values.len()).map_err(|_| failed())?;
            let next = start.checked_add(count).ok_or_else(failed)?;
            out.extend(values.iter().cloned());
            if page.get("isLast").and_then(Value::as_bool) == Some(true)
                || page
                    .get("total")
                    .and_then(Value::as_u64)
                    .is_some_and(|t| next >= t)
            {
                return Ok(out);
            }
            if count == 0 || page.get("maxResults").and_then(Value::as_u64) == Some(0) {
                return Err(failed());
            }
            start = next;
        }
        Err(failed())
    }
    pub(super) async fn read_metadata(&self, account: &str) -> Result<Metadata> {
        // /field is intentionally the documented non-paginated visible-field API;
        // unlike /field/search it does not require Jira administrator permission.
        let fields: Vec<Field> = self
            .get("rest/api/3/field", &[])
            .await
            .map_err(discovery_error)?;
        let sprint_field = unique(fields.iter().filter(|f| sprint(f)))?.map(|f| f.id.clone());
        let team_field = unique(
            fields
                .iter()
                .filter(|f| f.name == self.config.team_field_name),
        )?;
        let board: Value = self
            .get(
                &format!(
                    "rest/agile/1.0/board/{}/configuration",
                    self.config.board_id
                ),
                &[],
            )
            .await
            .map_err(discovery_error)?;
        if board.get("id").and_then(Value::as_u64) != Some(self.config.board_id) {
            return Err(failed());
        }
        let estimation = board
            .pointer("/estimation/field/fieldId")
            .and_then(Value::as_str);
        let estimation_field = unique(fields.iter().filter(|f| Some(f.id.as_str()) == estimation))?;
        let mut metadata = Metadata {
            account_id: account.into(),
            projects: vec![],
            types: BTreeMap::new(),
            sprint_field,
            assignees: BTreeMap::new(),
            sprints: vec![],
            assignee_groups: self.config.assignee_groups.clone(),
            team: self.config.team.clone(),
            policies: self
                .config
                .projects
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        [
                            self.config.policy(key, Purpose::Implementation),
                            self.config.policy(key, Purpose::Review),
                        ],
                    )
                })
                .collect(),
        };
        for key in &self.config.projects {
            let project: Value = self
                .get(&format!("rest/api/3/project/{key}"), &[])
                .await
                .map_err(discovery_error)?;
            if project.get("key").and_then(Value::as_str) != Some(key) {
                return Err(failed());
            }
            let name = project
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .ok_or_else(failed)?;
            let permissions: Value = self
                .get(
                    "rest/api/3/mypermissions",
                    &[
                        ("projectKey", key.clone()),
                        (
                            "permissions",
                            "CREATE_ISSUES,EDIT_ISSUES,SCHEDULE_ISSUES,ASSIGN_ISSUES".into(),
                        ),
                    ],
                )
                .await
                .map_err(discovery_error)?;
            let permitted = |p: &str| {
                permissions
                    .pointer(&format!("/permissions/{p}/havePermission"))
                    .and_then(Value::as_bool)
                    == Some(true)
            };
            let follow_up = board.get("type").and_then(Value::as_str) == Some("scrum")
                && metadata.sprint_field.is_some()
                && permitted("EDIT_ISSUES")
                && permitted("SCHEDULE_ISSUES");
            let types = self
                .values(
                    &format!("rest/api/3/issue/createmeta/{key}/issuetypes"),
                    &[],
                )
                .await?;
            let mut names = BTreeSet::new();
            let mut type_ids = BTreeSet::new();
            let status_groups: Vec<Value> = self
                .get(&format!("rest/api/3/project/{key}/statuses"), &[])
                .await
                .map_err(discovery_error)?;
            for t in types {
                let id = t
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(failed)?;
                let type_name = t
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(failed)?;
                if !names.insert(type_name.to_owned()) || !type_ids.insert(id.to_owned()) {
                    return Err(ambiguous());
                }
                let matches: Vec<_> = status_groups
                    .iter()
                    .filter(|group| group.get("id").and_then(Value::as_str) == Some(id))
                    .collect();
                if matches.len() > 1 {
                    return Err(ambiguous());
                }
                let group = matches.first().ok_or_else(failed)?;
                if group.get("name").and_then(Value::as_str) != Some(type_name) {
                    return Err(ambiguous());
                }
                let mut statuses = BTreeMap::<String, StatusChoice>::new();
                for status in group
                    .get("statuses")
                    .and_then(Value::as_array)
                    .ok_or_else(failed)?
                {
                    let status: StatusChoice =
                        serde_json::from_value(status.clone()).map_err(|_| failed())?;
                    if status.id.trim().is_empty() || status.name.trim().is_empty() {
                        return Err(failed());
                    }
                    if statuses
                        .get(&status.id)
                        .is_some_and(|old| old.name != status.name)
                    {
                        return Err(ambiguous());
                    }
                    statuses.insert(status.id.clone(), status);
                }
                let mut statuses: Vec<_> = statuses.into_values().collect();
                statuses.sort_by(|a, b| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then_with(|| a.id.cmp(&b.id))
                });
                let values = self
                    .values(
                        &format!("rest/api/3/issue/createmeta/{key}/issuetypes/{id}"),
                        &[],
                    )
                    .await?;
                let create_fields: Vec<Field> = values
                    .into_iter()
                    .map(|mut v| {
                        // Jira field metadata may contain both fieldId and key, but no id.
                        let id = v.get("fieldId").or_else(|| v.get("key")).cloned();
                        if let Some(o) = v.as_object_mut() {
                            o.remove("fieldId");
                            o.remove("key");
                            if let Some(id) = id {
                                o.insert("id".into(), id);
                            }
                        }
                        serde_json::from_value(v).map_err(|_| failed())
                    })
                    .collect::<Result<_>>()?;
                let mut ids = BTreeSet::new();
                if create_fields.iter().any(|f| !ids.insert(&f.id)) {
                    return Err(ambiguous());
                }
                let points =
                    estimation_field.and_then(|f| create_fields.iter().find(|c| c.id == f.id));
                let team = team_field.and_then(|f| create_fields.iter().find(|c| c.id == f.id));
                let sprint = unique(create_fields.iter().filter(|f| sprint(f)))?;
                let mut selected = TypeMetadata {
                    id: id.into(),
                    points_field: None,
                    sprint_field: None,
                    team_field: None,
                    team_values: BTreeMap::new(),
                    teams: vec![],
                    statuses,
                    capabilities: CreateCapabilities {
                        can_create: false,
                        unassigned_allowed: false,
                        story_points: capability(
                            "unsupported",
                            points.is_some_and(|f| f.required),
                            "story_points",
                        ),
                        team: capability("unsupported", team.is_some_and(|f| f.required), "team"),
                        sprint: capability(
                            if follow_up {
                                "follow_up"
                            } else {
                                "unsupported"
                            },
                            sprint.is_some_and(|f| f.required),
                            "sprint",
                        ),
                        blockers: vec![],
                    },
                    create_fields: create_fields
                        .iter()
                        .map(|f| (f.id.clone(), f.clone()))
                        .collect(),
                };
                if let Some(f) = points {
                    if schema(f, "type", "number") && writable(f) {
                        selected.points_field = Some(f.id.clone());
                        selected.capabilities.story_points =
                            capability("create", f.required, "story_points");
                    } else {
                        selected.capabilities.story_points =
                            capability("unknown", f.required, "story_points");
                    }
                }
                if let Some(f) = sprint {
                    if schema(f, "type", "array") && schema(f, "items", "json") && writable(f) {
                        // Jira's gh-sprint create setter accepts the numeric sprint ID
                        // despite the read schema being an array of sprint objects.
                        selected.sprint_field = Some(f.id.clone());
                        selected.capabilities.sprint = capability("create", f.required, "sprint");
                    } else if !follow_up {
                        selected.capabilities.sprint = capability("unknown", f.required, "sprint");
                    }
                }
                if let Some(f) = team {
                    if schema(f, "type", "option")
                        && schema(
                            f,
                            "custom",
                            "com.atlassian.jira.plugin.system.customfieldtypes:select",
                        )
                        && writable(f)
                    {
                        selected.team_field = Some(f.id.clone());
                        selected.capabilities.team = capability("create", f.required, "team");
                        for option in f.allowed_values.as_ref().ok_or_else(failed)? {
                            if option.get("disabled").and_then(Value::as_bool) == Some(true) {
                                continue;
                            }
                            let option_id = option
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .ok_or_else(failed)?;
                            let name = option
                                .get("value")
                                .and_then(Value::as_str)
                                .filter(|n| !n.is_empty())
                                .ok_or_else(failed)?;
                            // Selection keys scope the option to the verified principal,
                            // project, type and field, not a label supplied by the client.
                            let mut hash = std::hash::DefaultHasher::new();
                            (
                                self.config.base_url.trim_end_matches('/'),
                                account,
                                key,
                                id,
                                &f.id,
                                option_id,
                                name,
                            )
                                .hash(&mut hash);
                            let choice = format!("team-{:016x}", hash.finish());
                            if selected
                                .team_values
                                .insert(choice.clone(), json!({"id":option_id}))
                                .is_some()
                            {
                                return Err(ambiguous());
                            }
                            selected.teams.push(TeamChoice {
                                id: choice,
                                name: name.into(),
                            });
                        }
                    } else {
                        // Atlassian Team is not a select. No reliable UUID discovery
                        // mechanism is implemented; never reuse select option IDs.
                        selected.capabilities.team = capability("unsupported", f.required, "team");
                    }
                }
                let assignee = create_fields.iter().find(|f| f.id == "assignee");
                selected.capabilities.unassigned_allowed = assignee
                    .is_some_and(|f| !f.required && writable(f))
                    && permitted("ASSIGN_ISSUES");
                if assignee.is_none_or(|f| !writable(f)) || !permitted("ASSIGN_ISSUES") {
                    selected.capabilities.blockers.push(diagnostic("assignee_unsupported", "assignee_id", "Explicit assignment or unassignment is not supported for this project and type."));
                }
                if !permitted("CREATE_ISSUES") {
                    selected.capabilities.blockers.push(diagnostic(
                        "create_permission_missing",
                        "issue_type",
                        "The current account cannot create issues in this project.",
                    ));
                }
                let handled: BTreeSet<&str> = [
                    // Core fields are checked independently for missing setters.
                    Some("project"),
                    Some("issuetype"),
                    Some("summary"),
                    Some("description"),
                    assignee.map(|f| f.id.as_str()),
                    selected.points_field.as_deref(),
                    selected.team_field.as_deref(),
                    selected.sprint_field.as_deref(),
                ]
                .into_iter()
                .flatten()
                .collect();
                for field in ["project", "issuetype", "summary"] {
                    if !selected.permits(field) {
                        selected.capabilities.blockers.push(diagnostic(
                            "required_field_unsupported",
                            "issue_type",
                            "A core create field is missing or not writable for this type.",
                        ));
                    }
                }
                for f in &create_fields {
                    if f.required
                        && !f.has_default
                        && (!handled.contains(f.id.as_str()) || !writable(f))
                    {
                        selected.capabilities.blockers.push(diagnostic("required_field_unsupported", "issue_type", "An upstream required field has no supported editor value or proven default."));
                    }
                }
                selected.capabilities.can_create = selected.capabilities.blockers.is_empty();
                selected
                    .teams
                    .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
                metadata
                    .types
                    .entry(key.clone())
                    .or_default()
                    .insert(type_name.into(), selected);
            }
            metadata.projects.push(Project {
                key: key.clone(),
                name: name.into(),
                issue_types: names.into_iter().collect(),
            });
        }
        Ok(metadata)
    }
    pub(super) async fn all_human_accounts(&self) -> Result<BTreeSet<String>> {
        let mut seen = BTreeSet::new();
        let mut candidates = BTreeSet::new();
        let mut start = 0u64;
        for _ in 0..self.limits.max_pages {
            // Get all users, not /user/search or the filtered first-1000
            // assignable-search range. Inactive/app accounts still advance the
            // unfiltered pagination offset, but never enter the eligible pool.
            let users: Vec<User> = self
                .get(
                    "rest/api/3/users/search",
                    &[
                        ("startAt", start.to_string()),
                        ("maxResults", self.limits.page_size.to_string()),
                    ],
                )
                .await
                .map_err(discovery_error)?;
            if users.is_empty() {
                return Ok(candidates);
            }
            let count = u64::try_from(users.len()).map_err(|_| failed())?;
            if count > u64::from(self.limits.page_size) {
                return Err(failed());
            }
            let previous = seen.len();
            for user in users {
                if user.account_id.trim().is_empty() {
                    return Err(failed());
                }
                seen.insert(user.account_id.clone());
                if user.eligible() {
                    candidates.insert(user.account_id);
                }
            }
            if previous == seen.len() {
                return Err(failed());
            }
            start = start.checked_add(count).ok_or_else(failed)?;
        }
        // Reaching the page bound without an end page is not completeness.
        Err(failed())
    }
    pub(super) async fn group_members(&self) -> Result<Option<BTreeSet<String>>> {
        let groups: BTreeSet<_> = self.config.assignee_groups.iter().collect();
        if groups.is_empty() {
            return Ok(None);
        }
        let mut members = BTreeSet::new();
        for name in groups {
            let matches = self
                .values("rest/api/3/group/bulk", &[("groupName", name.clone())])
                .await?;
            let exact: Vec<_> = matches
                .iter()
                .filter(|g| g.get("name").and_then(Value::as_str) == Some(name))
                .collect();
            if exact.len() != 1 {
                return Err(failed());
            }
            let id = exact[0]
                .get("groupId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(failed)?;
            let users = self
                .values(
                    "rest/api/3/group/member",
                    &[
                        ("groupId", id.into()),
                        ("includeInactiveUsers", "false".into()),
                    ],
                )
                .await?;
            for u in users {
                let u: User = serde_json::from_value(u).map_err(|_| failed())?;
                if u.account_id.trim().is_empty() {
                    return Err(failed());
                }
                if u.eligible() {
                    members.insert(u.account_id);
                }
            }
        }
        Ok(Some(members))
    }
    pub async fn options(
        &self,
        key: &str,
        purpose: Purpose,
        issue_type: Option<&str>,
        query: &str,
    ) -> Result<JiraOptions> {
        self.project(key)?;
        let metadata = self.choices().await?;
        self.resolve_options(&metadata, key, purpose, issue_type, query)
            .await
    }
    pub(super) async fn resolve_options(
        &self,
        metadata: &Metadata,
        key: &str,
        purpose: Purpose,
        issue_type: Option<&str>,
        query: &str,
    ) -> Result<JiraOptions> {
        let me = &metadata.account_id;
        let project = metadata
            .projects
            .iter()
            .find(|p| p.key == key)
            .ok_or_else(invalid_choice)?;
        let policy = &metadata.policies.get(key).ok_or_else(failed)?
            [usize::from(purpose == Purpose::Review)];
        let mut diagnostics = vec![];
        let default_type = project
            .issue_types
            .contains(&policy.issue_type)
            .then_some(policy.issue_type.clone());
        if default_type.is_none() {
            diagnostics.push(diagnostic(
                "issue_type_default_unavailable",
                "issue_type",
                "The configured purpose default issue type is not available.",
            ));
        }
        let selected_type = issue_type
            .map(str::to_owned)
            .or_else(|| default_type.clone());
        if selected_type
            .as_ref()
            .is_some_and(|t| !project.issue_types.contains(t))
        {
            return Err(invalid_choice());
        }
        let pool: Vec<_> = metadata
            .assignees
            .get(key)
            .ok_or_else(failed)?
            .iter()
            .filter(|p| purpose != Purpose::Review || &p.id != me)
            .cloned()
            .collect();
        let sprints = metadata.sprints.clone();
        let selected = selected_type
            .as_ref()
            .and_then(|t| metadata.types.get(key).and_then(|types| types.get(t)));
        let (teams, capabilities) = selected
            .map(|s| (s.teams.clone(), s.capabilities.clone()))
            .unwrap_or_else(|| {
                (
                    vec![],
                    CreateCapabilities {
                        can_create: false,
                        unassigned_allowed: false,
                        story_points: capability("unknown", false, "story_points"),
                        team: capability("unknown", false, "team"),
                        sprint: capability("unknown", false, "sprint"),
                        blockers: vec![diagnostic(
                            "issue_type_required",
                            "issue_type",
                            "Choose an available issue type.",
                        )],
                    },
                )
            });
        let assignee_id = None;
        if selected.is_some() && !capabilities.unassigned_allowed {
            diagnostics.push(diagnostic(
                "assignee_required",
                "assignee_id",
                "Choose an eligible assignee; unassigned is not supported.",
            ));
        }
        for (field, cap) in [
            ("sprint", &capabilities.sprint),
            ("story_points", &capabilities.story_points),
        ] {
            if cap.required {
                diagnostics.push(diagnostic(
                    &format!("{field}_required"),
                    if field == "sprint" {
                        "sprint_id"
                    } else {
                        field
                    },
                    "Choose a value for this Jira-required field.",
                ));
            }
        }
        let matches: Vec<_> = teams.iter().filter(|t| t.name == metadata.team).collect();
        let team = if matches.len() == 1 {
            Some(matches[0].id.clone())
        } else {
            if capabilities.team.required {
                diagnostics.push(diagnostic(if matches.len() > 1 { "team_default_ambiguous" } else { "team_default_unavailable" }, "team", "The configured semantic team default does not resolve uniquely; choose explicitly."));
            }
            None
        };
        let sprint_id = None;
        let query = query.to_lowercase();
        if query.chars().count() > 200 {
            return Err(Error::invalid());
        }
        Ok(JiraOptions {
            project_key: key.into(),
            purpose,
            issue_type: selected_type,
            assignees: pool
                .into_iter()
                .filter(|p| {
                    p.display_name.to_lowercase().contains(&query)
                        || p.username.to_lowercase().contains(&query)
                        || p.email
                            .as_ref()
                            .is_some_and(|email| email.to_lowercase().contains(&query))
                })
                .collect(),
            sprints,
            issue_types: project.issue_types.clone(),
            teams,
            statuses: selected.map(|s| s.statuses.clone()).unwrap_or_default(),
            defaults: JiraDefaults {
                issue_type: default_type,
                assignee_id,
                team,
                sprint_id,
                workflow: policy.workflow.clone().unwrap_or_default(),
            },
            capabilities,
            diagnostics,
        })
    }
    pub(super) fn validate_choices(&self, r: &CreateIssueRequest, o: &JiraOptions) -> Result<()> {
        if r.status
            .as_ref()
            .is_some_and(|name| o.statuses.iter().filter(|s| &s.name == name).count() != 1)
        {
            return Err(invalid_choice());
        }
        if !o.capabilities.can_create {
            return Err(Error::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "create_blocked",
                "Required Jira fields or permissions prevent creation.",
            ));
        }
        if r.assignee_id
            .as_ref()
            .is_some_and(|id| !o.assignees.iter().any(|u| &u.id == id))
        {
            return Err(invalid_choice());
        }
        if r.assignee_id.is_none() && !o.capabilities.unassigned_allowed {
            return Err(Error::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "create_blocked",
                "Choose an eligible assignee; Jira does not permit unassigned creation.",
            ));
        }
        if r.team
            .as_ref()
            .is_some_and(|id| !o.teams.iter().any(|t| &t.id == id))
        {
            return Err(invalid_choice());
        }
        for (present, cap) in [
            (r.story_points.is_some(), &o.capabilities.story_points),
            (r.team.is_some(), &o.capabilities.team),
            (r.sprint_id.is_some(), &o.capabilities.sprint),
        ] {
            if present && !cap.supported() {
                return Err(invalid_choice());
            }
            if !present && cap.required {
                return Err(Error::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "create_blocked",
                    "A Jira-required editor value is missing.",
                ));
            }
        }
        Ok(())
    }
}
