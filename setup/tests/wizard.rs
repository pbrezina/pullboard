use anyhow::{Result, bail};
use clap::Parser;
use pullboard_core::{
    config::{AiConfig, Config, ForgeConfig, JiraConfig, ProjectOverride, Secret},
    model::Kind,
};
use pullboard_setup::{
    cli::Cli,
    discovery::{BoardTeams, Catalog, Metadata, Sprint, TeamOption, TypeMetadata},
    prompt::{Back, Cancelled, Choice, OpenMenu, Prompts},
    wizard::{Discovery, Wizard, credential, next_source_id, project_cleanup},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
};
use tokio_util::sync::CancellationToken;

fn temp_home() -> tempfile::TempDir {
    if std::path::Path::new("/tmp/opencode").is_dir() {
        tempfile::tempdir_in("/tmp/opencode")
    } else {
        tempfile::tempdir()
    }
    .unwrap()
}

enum Step {
    Enter(&'static str),
    Text(&'static str, &'static str),
    Password(bool, &'static str),
    Confirm(&'static str, bool),
    Select(&'static str, usize),
    Multi(&'static str, Vec<usize>),
    Cancel(&'static str),
    Escape(&'static str),
    Menu(&'static str),
}
use Step::*;
#[derive(Default)]
struct Script {
    steps: VecDeque<Step>,
    transcript: Vec<String>,
    defaults: Vec<(String, String)>,
    options: Vec<(String, Vec<Choice>)>,
    local_cancel: bool,
}
impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            ..Self::default()
        }
    }
    fn next(&mut self, message: &str) -> Result<Step> {
        let step = self
            .steps
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected prompt: {message}"));
        let expected = match &step {
            Enter(q)
            | Text(q, _)
            | Confirm(q, _)
            | Select(q, _)
            | Multi(q, _)
            | Cancel(q)
            | Escape(q)
            | Menu(q) => *q,
            Password(_, _) => "Token",
        };
        assert!(
            message.starts_with(expected),
            "expected {expected}, got {message}"
        );
        if matches!(step, Cancel(_)) {
            return Err(Cancelled.into());
        }
        if matches!(step, Escape(_)) {
            assert!(self.local_cancel);
            return Err(Back.into());
        }
        if matches!(step, Menu(_)) {
            return Err(OpenMenu.into());
        }
        Ok(step)
    }
    fn done(&self) {
        assert!(self.steps.is_empty(), "unasked questions remain");
    }
}
impl Prompts for Script {
    fn local_cancel(&mut self, enabled: bool) {
        self.local_cancel = enabled;
    }
    fn say(&mut self, s: &str) {
        self.transcript.push(s.into());
    }
    async fn text(&mut self, q: &str, d: &str) -> Result<String> {
        self.defaults.push((q.into(), d.into()));
        let v = match self.next(q)? {
            Enter(_) => return Ok(d.into()),
            Text(_, v) => v,
            _ => panic!("expected text"),
        };
        Ok(if v == "<default>" { d.into() } else { v.into() })
    }
    async fn password(&mut self, saved: bool) -> Result<String> {
        let Password(expected, v) = self.next("Token")? else {
            panic!("expected password")
        };
        assert_eq!(saved, expected);
        Ok(v.into())
    }
    async fn confirm(&mut self, q: &str, d: bool) -> Result<bool> {
        self.defaults.push((q.into(), d.to_string()));
        let v = match self.next(q)? {
            Enter(_) => d,
            Confirm(_, v) => v,
            _ => panic!("expected confirm"),
        };
        Ok(v)
    }
    async fn select(&mut self, q: &str, c: &[Choice], d: usize) -> Result<usize> {
        self.options.push((q.into(), c.to_vec()));
        self.defaults.push((q.into(), d.to_string()));
        let v = match self.next(q)? {
            Enter(_) => d,
            Select(_, v) => v,
            _ => panic!("expected select"),
        };
        assert!(v < c.len());
        Ok(v)
    }
    async fn multi(&mut self, q: &str, c: &[Choice], d: &[usize]) -> Result<Vec<usize>> {
        self.options.push((q.into(), c.to_vec()));
        self.defaults.push((q.into(), format!("{d:?}")));
        let v = match self.next(q)? {
            // Inquire returns multiselect answers in original inventory order,
            // not in the saved project's primary-first configuration order.
            Enter(_) => d.to_vec(),
            Multi(_, v) => v,
            _ => panic!("expected multi"),
        };
        assert!(v.iter().all(|i| *i < c.len()));
        Ok(v)
    }
}

#[derive(Default)]
struct MockDiscovery {
    projects: Vec<Choice>,
    failures: RefCell<VecDeque<bool>>,
    catalogs: Cell<usize>,
    identity_failures: RefCell<VecDeque<bool>>,
    empty_projects: bool,
    empty_boards: bool,
    board_failures: RefCell<VecDeque<bool>>,
    manual_projects: BTreeMap<String, Vec<Choice>>,
    manual_boards: BTreeMap<String, Choice>,
    all_projects: Vec<Choice>,
    project_boards: Vec<Choice>,
    broad_failure: bool,
    manual_error: Option<&'static str>,
    own_groups: Option<Vec<Choice>>,
    own_groups_fail: bool,
    all_groups: Vec<Choice>,
    manual_groups: BTreeMap<String, Choice>,
    calls: RefCell<Vec<String>>,
    forge_failures: RefCell<VecDeque<bool>>,
    forges: RefCell<Vec<ForgeConfig>>,
    ai_calls: Cell<usize>,
    ai_fails: bool,
    ambiguous_sprints: bool,
    metadata_teams: Option<Vec<String>>,
    board_teams: Option<Vec<String>>,
    team_failures: RefCell<VecDeque<bool>>,
}
impl Discovery for MockDiscovery {
    async fn catalog(&self, _: &JiraConfig) -> Result<Catalog> {
        self.calls.borrow_mut().push("recents".into());
        self.catalogs.set(self.catalogs.get() + 1);
        if self.failures.borrow_mut().pop_front().unwrap_or(false) {
            bail!("recent projects returned HTTP 403; check Browse Projects permission");
        }
        Ok(Catalog {
            identity: "Mock human".into(),
            projects: if self.empty_projects {
                vec![]
            } else if self.projects.is_empty() {
                vec![Choice::new("ONE", "ONE Mock project")]
            } else {
                self.projects.clone()
            },
            groups: vec![],
            fields: vec![],
        })
    }
    async fn identity(&self, _: &JiraConfig) -> Result<String> {
        self.calls.borrow_mut().push("identity".into());
        if self
            .identity_failures
            .borrow_mut()
            .pop_front()
            .unwrap_or(false)
        {
            bail!("verifying Jira identity: HTTP 401; check the account email and API token");
        }
        Ok("Mock human".into())
    }
    async fn boards(&self, _: &JiraConfig) -> Result<Vec<Choice>> {
        self.calls.borrow_mut().push("boards".into());
        if self
            .board_failures
            .borrow_mut()
            .pop_front()
            .unwrap_or(false)
        {
            bail!("recent boards returned HTTP 404; recent endpoint unavailable");
        }
        Ok(if self.empty_boards {
            vec![]
        } else {
            vec![Choice::new("7", "7 Mock board")]
        })
    }
    async fn all_projects(&self, _: &JiraConfig) -> Result<Vec<Choice>> {
        self.calls.borrow_mut().push("all_projects".into());
        if self.broad_failure {
            bail!("all-project pagination incomplete: max_pages 100 reached");
        }
        Ok(self.all_projects.clone())
    }
    async fn project(&self, _: &JiraConfig, input: &str) -> Result<Vec<Choice>> {
        self.calls.borrow_mut().push(format!("project:{input}"));
        if let Some(message) = self.manual_error {
            bail!("{message}");
        }
        self.manual_projects
            .get(input)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("project not found or not permitted (HTTP 404)"))
    }
    async fn board(&self, _: &JiraConfig, input: &str) -> Result<Choice> {
        self.calls.borrow_mut().push(format!("board:{input}"));
        if let Some(message) = self.manual_error {
            bail!("{message}");
        }
        self.manual_boards
            .get(input)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("board not found or not permitted (HTTP 404)"))
    }
    async fn project_boards(&self, j: &JiraConfig) -> Result<Vec<Choice>> {
        self.calls
            .borrow_mut()
            .push(format!("project_boards:{}", j.projects.join(",")));
        if self.broad_failure {
            bail!("selected-project board pagination incomplete: max_pages 100 reached");
        }
        Ok(self.project_boards.clone())
    }
    async fn settings(&self, j: &JiraConfig) -> Result<Catalog> {
        assert!(!j.projects.is_empty());
        assert!(j.board_id > 0);
        self.calls.borrow_mut().push("settings".into());
        Ok(Catalog {
            groups: vec![Choice::new("developers", "developers")],
            fields: vec![Choice::new("AssignedTeam", "AssignedTeam")],
            ..Default::default()
        })
    }
    async fn own_groups(&self, _: &JiraConfig) -> Result<Vec<Choice>> {
        self.calls.borrow_mut().push("own_groups".into());
        if self.own_groups_fail {
            bail!("user groups returned HTTP 403");
        }
        Ok(self
            .own_groups
            .clone()
            .unwrap_or_else(|| vec![Choice::new("developers", "developers")]))
    }
    async fn all_groups(&self, _: &JiraConfig) -> Result<Vec<Choice>> {
        self.calls.borrow_mut().push("all_groups".into());
        if self.broad_failure {
            bail!("all-group pagination incomplete");
        }
        Ok(self.all_groups.clone())
    }
    async fn group(&self, _: &JiraConfig, name: &str) -> Result<Choice> {
        self.calls.borrow_mut().push(format!("group:{name}"));
        self.manual_groups
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("group not found or member read forbidden (HTTP 403)"))
    }
    async fn metadata(&self, j: &JiraConfig) -> Result<Metadata> {
        self.calls.borrow_mut().push("metadata".into());
        let mut sprints = vec![
            Sprint {
                id: "1".into(),
                name: "Team A current".into(),
                state: "active".into(),
            },
            Sprint {
                id: "2".into(),
                name: "Team A future".into(),
                state: "future".into(),
            },
        ];
        if self.ambiguous_sprints {
            sprints.push(Sprint {
                id: "3".into(),
                name: "Team A second".into(),
                state: "active".into(),
            });
        }
        Ok(Metadata {
            board_id: j.board_id,
            team_field_name: j.team_field_name.clone(),
            team_field_id: Some("customfield_1".into()),
            types: j
                .projects
                .iter()
                .map(|project| TypeMetadata {
                    project: project.clone(),
                    name: "Task".into(),
                    teams: self
                        .metadata_teams
                        .clone()
                        .unwrap_or_else(|| vec!["Team A".into()]),
                    team_options: self
                        .metadata_teams
                        .clone()
                        .unwrap_or_else(|| vec!["Team A".into()])
                        .into_iter()
                        .enumerate()
                        .map(|(i, value)| TeamOption {
                            id: (i + 100).to_string(),
                            value,
                        })
                        .collect(),
                    statuses: vec!["Open".into(), "Ready".into()],
                    notes: vec![],
                })
                .collect(),
            sprints,
        })
    }
    async fn board_teams(&self, j: &JiraConfig, m: &Metadata) -> Result<BoardTeams> {
        assert_eq!(j.board_id, m.board_id);
        self.calls
            .borrow_mut()
            .push(format!("board_teams:{}", j.board_id));
        if self.team_failures.borrow_mut().pop_front().unwrap_or(false) {
            bail!("active-sprint issue pagination incomplete (HTTP 403); new-jira-secret");
        }
        Ok(BoardTeams {
            teams: self
                .board_teams
                .clone()
                .unwrap_or_else(|| vec!["Team A".into()]),
            unverified_values: 0,
        })
    }
    async fn forge(&self, f: &ForgeConfig) -> Result<String> {
        self.forges.borrow_mut().push(f.clone());
        if self
            .forge_failures
            .borrow_mut()
            .pop_front()
            .unwrap_or(false)
        {
            bail!("synthetic failure");
        }
        Ok("Mock forge human".into())
    }
    async fn ai_test(&self, ai: &AiConfig) -> Result<()> {
        assert_eq!(ai.model, "gpt-6-luna");
        self.ai_calls.set(self.ai_calls.get() + 1);
        if self.ai_fails {
            bail!("do not print upstream response");
        }
        Ok(())
    }
}
fn app() -> Wizard {
    Wizard::new(Cli::try_parse_from(["setup"]).unwrap())
}
fn fixture() -> Config {
    Config::from_yaml("read_only_test_mode: true\nrefresh_interval_seconds: 1234\njira:\n  base_url: https://first.atlassian.net\n  email: human@example.invalid\n  token: {value: jira-sentinel-secret}\n  projects: [ONE, TWO]\n  board_id: 7\n  team: Team A\n  sprint_name_pattern: '^Team A'\n  assignee_groups: [developers]\nforges:\n  - id: stable-one\n    name: Personal\n    kind: github\n    base_url: https://api.github.com\n    token: {value: forge-sentinel-secret}\n    include_repositories: ['org/*']\n    default_project: TWO\nai:\n  token: {value: ai-sentinel-secret}\n").unwrap()
}
fn runtime() -> Vec<Step> {
    vec![
        Text("Local host port", "<default>"),
        Select("Container channel", 0),
        Confirm("Start in Test mode", false),
    ]
}
fn disabled_tail() -> Vec<Step> {
    let mut s = vec![
        Confirm("Add a forge account", false),
        Select("AI provider", 0),
    ];
    s.extend(runtime());
    s
}
fn jira_new() -> Vec<Step> {
    let mut steps = vec![
        Confirm("Configure Jira", true),
        Text("Jira Cloud tenant URL", "https://first.atlassian.net"),
        Text("Atlassian account email", "human@example.invalid"),
        Password(false, "new-jira-secret"),
    ];
    steps.extend(jira_selections());
    steps.extend([
        Text("Regular expression to identify your sprints", "<default>"),
        Confirm("Configure advanced issue", false),
    ]);
    steps
}
fn jira_selections() -> Vec<Step> {
    vec![
        Enter("Jira projects"),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
    ]
}
async fn configured(a: &mut Wizard, p: &mut Script, d: &MockDiscovery) {
    a.configure(p, d, &CancellationToken::new()).await.unwrap();
    p.done();
    a.config.validate().unwrap();
}

#[tokio::test]
async fn new_linear_flow_opens_singleton_pickers_directly_and_has_live_ai_disabled_defaults() {
    let mut a = app();
    assert!(!a.config.read_only_test_mode);
    assert!(Config::default().read_only_test_mode);
    let mut steps = jira_new();
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &MockDiscovery::default()).await;
    assert!(!a.config.read_only_test_mode);
    assert!(!a.config.ai.enabled);
    assert_eq!(a.config.ai.model, "gpt-6-luna");
    assert_eq!(a.config.jira.as_ref().unwrap().projects, ["ONE"]);
    assert_eq!(a.config.server.port, 8080);
    assert_eq!(
        a.config.server.allowed_origins,
        ["http://localhost:3333", "http://127.0.0.1:3333"]
    );
    assert!(
        p.transcript
            .iter()
            .any(|s| s.contains("only available choice"))
    );
    assert!(
        p.defaults
            .contains(&("Start in Test mode?".into(), "false".into()))
    );
    assert!(p.defaults.contains(&(
        "Configure advanced issue-type/workflow suggestions?".into(),
        "false".into()
    )));
    assert!(p.transcript.iter().all(|s| !s.contains("new-jira-secret")));
    for question in [
        "Jira projects:",
        "Jira board:",
        "Assignee groups (at least one):",
        "Team field (semantic name):",
        "Team:",
    ] {
        assert!(p.options.iter().any(|(q, _)| q == question));
    }
    assert!(!p.options.iter().any(|(q, _)| q.ends_with(" actions:")));
    assert_eq!(
        p.options
            .iter()
            .find(|(q, _)| q == "Team field (semantic name):")
            .unwrap()
            .1
            .len(),
        1
    );
    Config::from_yaml(&a.config.to_yaml().unwrap()).unwrap();
}

#[tokio::test]
async fn team_default_picker_contains_only_observed_verified_active_board_choices() {
    let mut a = app();
    let d = MockDiscovery {
        metadata_teams: Some(vec!["Team A".into(), "Team B".into()]),
        board_teams: Some(vec!["Team B".into(), "Unverified".into()]),
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    let items = &p.options.iter().find(|(q, _)| q == "Team:").unwrap().1;
    assert_eq!(
        items.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["Team B"]
    );
    assert_eq!(a.config.jira.as_ref().unwrap().team, "Team B");
    assert_eq!(a.config.jira.as_ref().unwrap().board_id, 7);
    assert!(
        !d.calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("project_boards"))
    );
}

#[tokio::test]
async fn saved_verified_team_is_preserved_even_when_not_observed_on_the_active_board() {
    for valid_saved in [true, false] {
        let mut a = app();
        a.config = fixture();
        a.config.forges.clear();
        let j = a.config.jira.as_mut().unwrap();
        j.projects = vec!["ONE".into()];
        j.team = if valid_saved {
            "Team B"
        } else {
            "Removed team"
        }
        .into();
        let d = MockDiscovery {
            metadata_teams: Some(vec!["Team A".into(), "Team B".into()]),
            board_teams: Some(vec!["Team A".into()]),
            ..Default::default()
        };
        let mut steps = jira_new();
        steps[3] = Password(true, "");
        steps.extend(disabled_tail());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        let items = &p.options.iter().find(|(q, _)| q == "Team:").unwrap().1;
        assert_eq!(
            a.config.jira.as_ref().unwrap().team,
            if valid_saved { "Team B" } else { "Team A" }
        );
        assert_eq!(items.len(), if valid_saved { 2 } else { 1 });
        if valid_saved {
            assert!(items[1].label.contains("saved; verified"));
            assert!(p.defaults.contains(&("Team:".into(), "1".into())));
        }
    }
}

#[tokio::test]
async fn team_ctrl_o_offers_all_verified_metadata_choices_without_a_new_scan_or_board_change() {
    let mut a = app();
    let d = MockDiscovery {
        metadata_teams: Some(vec!["Team A".into(), "Team B".into()]),
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.truncate(8);
    steps.extend([
        Menu("Team:"),
        Select("Team actions", 0),
        Menu("Team:"),
        Select("Team actions", 1),
        Select("Team:", 1),
        Enter("Regular expression to identify your sprints"),
        Enter("Configure advanced issue"),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(a.config.jira.as_ref().unwrap().team, "Team B");
    assert_eq!(a.config.jira.as_ref().unwrap().board_id, 7);
    assert_eq!(
        d.calls
            .borrow()
            .iter()
            .filter(|call| call.as_str() == "metadata")
            .count(),
        1
    );
    assert_eq!(
        d.calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("board_teams:"))
            .count(),
        1
    );
    assert!(
        !d.calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("project_boards"))
    );
    assert_eq!(
        p.options
            .iter()
            .filter(|(q, _)| q == "Team:")
            .map(|(_, c)| c.len())
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
}

#[tokio::test]
async fn failed_or_empty_board_teams_open_explicit_fallback_and_retry_without_partial_or_broader_scan()
 {
    for failed in [true, false] {
        for retry in [true, false] {
            let mut a = app();
            let d = MockDiscovery {
                board_teams: Some(if failed {
                    vec!["Team A".into()]
                } else {
                    vec![]
                }),
                ..Default::default()
            };
            if failed {
                d.team_failures.borrow_mut().push_back(true);
            }
            let mut steps = jira_new();
            steps.truncate(8);
            steps.push(Select("Team actions", 0)); // Cannot expose a partial/empty board list.
            if retry {
                steps.push(Select("Team actions", 2));
            }
            if !retry || !failed {
                steps.push(Select("Team actions", 1));
            }
            steps.extend([
                Enter("Team:"),
                Enter("Regular expression to identify your sprints"),
                Enter("Configure advanced issue"),
            ]);
            steps.extend(disabled_tail());
            let mut p = Script::new(steps);
            configured(&mut a, &mut p, &d).await;
            assert_eq!(a.config.jira.as_ref().unwrap().team, "Team A");
            let calls = d.calls.borrow();
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call.as_str() == "metadata")
                    .count(),
                1
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call.starts_with("board_teams:"))
                    .count(),
                if retry { 2 } else { 1 }
            );
            assert!(
                !calls
                    .iter()
                    .any(|call| call.starts_with("all_") || call.starts_with("project_boards"))
            );
            let log = p.transcript.join("\n");
            assert!(!log.contains("new-jira-secret"));
            assert!(log.contains(if failed {
                "no partial list"
            } else {
                "No verified teams were observed"
            }));
        }
    }
}

#[tokio::test]
async fn explicit_singleton_team_field_can_be_cancelled_before_metadata_and_does_not_change_config()
{
    let mut a = app();
    let before = a.config.to_yaml().unwrap();
    let d = MockDiscovery::default();
    let mut steps = jira_new();
    steps.truncate(7);
    steps.push(Cancel("Team field (semantic name):"));
    let mut p = Script::new(steps);
    assert!(
        a.configure(&mut p, &d, &CancellationToken::new())
            .await
            .unwrap_err()
            .is::<Cancelled>()
    );
    p.done();
    assert_eq!(a.config.to_yaml().unwrap().as_str(), before.as_str());
    assert!(
        !d.calls
            .borrow()
            .iter()
            .any(|call| call == "metadata" || call.starts_with("board_teams"))
    );
}

#[tokio::test]
async fn failed_jira_only_retries_changes_or_removes_never_saves_unverified() {
    for action in [0, 1, 2] {
        let mut a = app();
        let d = MockDiscovery::default();
        d.failures.borrow_mut().push_back(true);
        d.identity_failures.borrow_mut().push_back(true);
        let mut steps = jira_new();
        steps.truncate(4);
        steps.push(Select("Connection failed", action));
        if action == 1 {
            steps.extend([
                Text("Jira Cloud tenant URL", "<default>"),
                Text("Atlassian account email", "<default>"),
                Password(true, ""),
            ]);
        }
        if action != 2 {
            if action == 0 {
                steps.push(Select("Project actions", 3));
                steps.push(Select("Project actions", 0));
            }
            steps.extend(jira_selections());
            steps.extend([
                Text("Regular expression to identify your sprints", "<default>"),
                Confirm("Configure advanced issue", false),
            ]);
        }
        steps.extend(disabled_tail());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        assert_eq!(a.config.jira.is_some(), action != 2);
        assert_eq!(d.catalogs.get(), if action == 2 { 1 } else { 2 });
        assert!(p.transcript.join("\n").contains("HTTP 401"));
    }
}

#[tokio::test]
async fn edited_project_removal_is_deliberate_and_preserves_nonorphaned_advanced_settings() {
    let mut a = app();
    a.config = fixture();
    let j = a.config.jira.as_mut().unwrap();
    j.project_overrides
        .insert("ONE".into(), ProjectOverride::default());
    j.project_overrides
        .insert("TWO".into(), ProjectOverride::default());
    a.config.upstream.max_pages = 31;
    let mut steps = vec![
        Confirm("Configure Jira", true),
        Text("Jira Cloud tenant URL", "<default>"),
        Text("Atlassian account email", "<default>"),
        Password(true, ""),
        Multi("Jira projects", vec![0]),
        Confirm("Confirm project removal", false),
        Multi("Jira projects", vec![0]),
        Confirm("Confirm project removal", true),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Text("Regular expression to identify your sprints", "<default>"),
        Confirm("Configure advanced issue", false),
        Confirm("Configure existing forge", true),
        Select("Forge provider", 0),
        Text("Forge URL", "<default>"),
        Password(true, ""),
        Confirm("Configure advanced repository", false),
    ];
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    let d = MockDiscovery {
        projects: vec![Choice::new("ONE", "One"), Choice::new("TWO", "Two")],
        ..Default::default()
    };
    configured(&mut a, &mut p, &d).await;
    let j = a.config.jira.as_ref().unwrap();
    assert_eq!(j.projects, ["ONE"]);
    assert!(j.project_overrides.contains_key("ONE"));
    assert!(!j.project_overrides.contains_key("TWO"));
    assert!(a.config.forges[0].default_project.is_none());
    assert_eq!(a.config.forges[0].include_repositories, ["org/*"]);
    assert_eq!(a.config.forges[0].id, "stable-one");
    assert_eq!(a.config.refresh_interval_seconds, 1234);
    assert_eq!(a.config.upstream.max_pages, 31);
    assert!(
        p.defaults
            .contains(&("Start in Test mode?".into(), "true".into()))
    );
    assert!(
        matches!(&a.config.jira.as_ref().unwrap().token,Secret::Literal{value} if value=="jira-sentinel-secret")
    );
    assert!(!a.summary().join("\n").contains("sentinel-secret"));
}

#[tokio::test]
async fn enter_keeps_tokens_but_changed_destination_requires_consent_or_reentry() {
    let saved = Secret::Literal {
        value: "saved-sentinel".into(),
    };
    for changed in [false, true] {
        let mut steps = vec![Password(true, "")];
        if changed {
            steps.push(Confirm("Send the saved", true));
        }
        let mut p = Script::new(steps);
        let s = credential(&mut p, Some(&saved), changed).await.unwrap();
        p.done();
        assert!(matches!(s,Secret::Literal{value} if value=="saved-sentinel"));
    }
    let mut p = Script::new(vec![
        Password(true, ""),
        Confirm("Send the saved", false),
        Password(true, "replacement-sentinel"),
    ]);
    assert!(
        matches!(credential(&mut p,Some(&saved),true).await.unwrap(),Secret::Literal{value} if value=="replacement-sentinel")
    );
    p.done();
    let mut p = Script::new(vec![Password(true, "replacement")]);
    credential(&mut p, Some(&saved), true).await.unwrap();
    p.done();
}

#[tokio::test]
async fn jira_cli_prefill_never_sends_saved_token_without_explicit_destination_consent() {
    let mut a = Wizard::new(
        Cli::try_parse_from(["setup", "--jira-cloud-url", "https://second.atlassian.net"]).unwrap(),
    );
    a.config = fixture();
    a.config.forges.clear();
    let steps = vec![
        Confirm("Configure Jira", true),
        Text("Jira Cloud tenant URL", "<default>"),
        Text("Atlassian account email", "<default>"),
        Password(true, ""),
        Cancel("Send the saved"),
    ];
    let d = MockDiscovery::default();
    let mut p = Script::new(steps);
    assert!(
        a.configure(&mut p, &d, &CancellationToken::new())
            .await
            .unwrap_err()
            .is::<Cancelled>()
    );
    assert_eq!(d.catalogs.get(), 0);
    assert_eq!(
        a.config.jira.as_ref().unwrap().base_url,
        "https://first.atlassian.net"
    );
}

#[tokio::test]
async fn new_forge_is_provider_url_token_verify_then_add_another_no_default_with_stable_ids() {
    let mut a = app();
    let mut steps = vec![
        Confirm("Configure Jira", false),
        Confirm("Add a forge account", true),
        Select("Forge provider", 1),
        Text("Forge URL", "<default>"),
        Password(false, "synthetic-forge-token"),
        Confirm("Configure advanced repository", false),
        Confirm("Add another forge", false),
        Select("AI provider", 0),
    ];
    steps.extend(runtime());
    let d = MockDiscovery::default();
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(a.config.forges.len(), 1);
    let f = &a.config.forges[0];
    assert_eq!(f.kind, Kind::Gitlab);
    assert_eq!(f.id, "gitlab-1");
    assert_eq!(f.base_url, "https://gitlab.com");
    assert_eq!(next_source_id(Kind::Gitlab, &a.config.forges), "gitlab-2");
    assert!(
        p.defaults
            .contains(&("Add another forge?".into(), "false".into()))
    );
    assert_eq!(d.forges.borrow().len(), 1);
}

#[tokio::test]
async fn failed_existing_forge_skip_explicitly_removes_it_without_retaining_unverified_source() {
    let mut a = app();
    a.config = fixture();
    a.config.jira = None;
    a.config.forges[0].default_project = None;
    let mut steps = vec![
        Confirm("Configure Jira", false),
        Confirm("Configure existing forge", true),
        Select("Forge provider", 0),
        Text("Forge URL", "<default>"),
        Password(true, ""),
        Select("Connection failed", 2),
        Confirm("Remove this existing forge", true),
    ];
    steps.extend(disabled_tail());
    let d = MockDiscovery::default();
    d.forge_failures.borrow_mut().push_back(true);
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert!(a.config.forges.is_empty());
}

#[tokio::test]
async fn ai_privacy_and_separate_billing_consent_gate_exactly_one_synthetic_request() {
    for (privacy, test, billing, expected) in [
        (false, false, false, 0),
        (true, false, false, 0),
        (true, true, false, 0),
        (true, true, true, 1),
    ] {
        let mut a = app();
        let mut steps = vec![
            Confirm("Configure Jira", false),
            Confirm("Add a forge account", false),
            Select("AI provider", 1),
            Confirm("Allow selected PR", privacy),
        ];
        if privacy {
            steps.extend([
                Text("AI endpoint", "<default>"),
                Text("Exact model ID", "<default>"),
                Password(false, "synthetic-ai-token"),
                Confirm("Run an optional", test),
            ]);
            if test {
                steps.push(Confirm("Authorize one potentially", billing));
            }
        }
        steps.extend(runtime());
        let mut p = Script::new(steps);
        let d = MockDiscovery {
            ai_fails: true,
            ..Default::default()
        };
        configured(&mut a, &mut p, &d).await;
        assert_eq!(a.config.ai.enabled, privacy);
        assert_eq!(d.ai_calls.get(), expected);
        assert_eq!(a.config.ai.model, "gpt-6-luna");
        if expected == 1 {
            assert!(
                p.transcript
                    .iter()
                    .any(|s| s.contains("failed; editable deterministic"))
            );
        }
    }
}

#[tokio::test]
async fn ai_endpoint_change_requires_saved_token_consent_even_without_a_test() {
    let mut a = app();
    a.config = fixture();
    a.config.jira = None;
    a.config.forges.clear();
    let mut steps = vec![
        Confirm("Configure Jira", false),
        Confirm("Add a forge account", false),
        Select("AI provider", 1),
        Confirm("Allow selected PR", true),
        Text("AI endpoint", "https://ai.example.invalid/v1"),
        Text("Exact model ID", "<default>"),
        Password(true, ""),
        Confirm("Send the saved", true),
        Confirm("Run an optional", false),
    ];
    steps.extend(runtime());
    let d = MockDiscovery::default();
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(d.ai_calls.get(), 0);
    assert!(
        matches!(&a.config.ai.token,Some(Secret::Literal{value}) if value=="ai-sentinel-secret")
    );
}

#[tokio::test]
async fn advanced_scoped_suggestions_are_opt_in_and_never_assign_self() {
    let mut a = app();
    let mut steps = jira_new();
    steps.pop();
    steps.extend([
        Confirm("Configure advanced issue", true),
        Confirm("Edit suggestions for", true),
        Confirm("Edit Implementation", true),
        Select("Draft status", 0),
        Select("Ready status", 2),
        Confirm("Edit Review", false),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &MockDiscovery::default()).await;
    let policy = a.config.jira.as_ref().unwrap().project_overrides["ONE"]
        .defaults
        .implementation
        .as_ref()
        .unwrap();
    assert!(policy.assignee.is_none());
    assert_eq!(policy.issue_type, "Task");
    assert!(policy.workflow.as_ref().unwrap().draft.is_none());
    assert_eq!(
        policy.workflow.as_ref().unwrap().ready.as_deref(),
        Some("Ready")
    );
}

#[tokio::test]
async fn regex_search_defaults_do_not_filter_inventory_and_ambiguity_requires_acknowledgement() {
    let mut a = app();
    let mut steps = jira_new();
    steps.truncate(4);
    steps.extend(jira_selections());
    steps.extend([
        Text("Regular expression to identify your sprints", "["),
        Text("Regular expression to identify your sprints", "Team"),
        Confirm("Continue with no unique", false),
        Text(
            "Regular expression to identify your sprints",
            "Team A current",
        ),
        Confirm("Configure advanced issue", false),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    let d = MockDiscovery {
        ambiguous_sprints: true,
        ..Default::default()
    };
    configured(&mut a, &mut p, &d).await;
    let log = p.transcript.join("\n");
    assert!(log.contains("future — Team A future"));
    assert!(log.contains("2 matching active"));
    assert!(log.contains("1 matching active"));
}

#[tokio::test]
async fn summary_final_action_is_only_authorization_and_cancel_changes_no_files() {
    let a = app();
    let home = temp_home();
    for action in [0, 1] {
        let mut p = Script::new(vec![Select("Install:", action)]);
        assert_eq!(a.authorize(&mut p).await.unwrap(), action == 0);
        p.done();
        assert_eq!(p.defaults.last().unwrap().1, "0");
    }
    let mut p = Script::new(vec![Cancel("Install:")]);
    assert!(a.authorize(&mut p).await.unwrap_err().is::<Cancelled>());
    assert_eq!(home.path().read_dir().unwrap().count(), 0);
    assert!(!include_str!("../src/prompt.rs").contains("EnterAlternateScreen"));
    assert!(!include_str!("../src/wizard.rs").contains("xdg-open"));
}

#[test]
fn project_cleanup_and_terminal_label_sanitization_do_not_expose_tokens_or_commands() {
    let config = fixture();
    let lines = project_cleanup(&config, &["ONE".into()]).join("\n");
    assert!(lines.contains("TWO"));
    assert!(lines.contains("stable-one"));
    assert!(!lines.contains("sentinel-secret"));
    let text = pullboard_setup::prompt::clean("upstream\x1b]2;evil\x07\n\r\u{202e}name");
    assert!(!text.contains(['\x1b', '\x07', '\n', '\r', '\u{202e}']));
}

#[tokio::test]
async fn protected_inspection_preserves_existing_channel_port_mode_and_advanced_policy() {
    use pullboard_setup::install::{
        Channel, Command, CommandOutput, CommandRunner, HttpProbe, InstallError, Installer, Paths,
        render_quadlet,
    };
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    struct Prerequisites;
    impl CommandRunner for Prerequisites {
        async fn run(
            &self,
            command: Command,
            _: Duration,
            _: &CancellationToken,
        ) -> std::result::Result<CommandOutput, InstallError> {
            let stdout = match command {
                Command::PodmanInfo => {
                    r#"{"host":{"security":{"rootless":true},"cgroupVersion":"v2"}}"#
                }
                Command::ServiceStatus => "LoadState=not-found\n",
                Command::GraphicalStatus => "active",
                Command::ContainerInventory => "[]",
                Command::QuadletVersion | Command::UserManager => "available",
                _ => panic!("inspection must never modify anything"),
            };
            Ok(CommandOutput {
                success: true,
                stdout: stdout.into(),
            })
        }
    }
    for (existing, nightly, expected) in [
        (Channel::Nightly, false, Channel::Nightly),
        (Channel::Stable, false, Channel::Stable),
        (Channel::Stable, true, Channel::Nightly),
    ] {
        let home = temp_home();
        let paths = Paths::for_home(home.path().into()).unwrap();
        std::fs::create_dir_all(paths.config().parent().unwrap()).unwrap();
        std::fs::create_dir_all(paths.quadlet().parent().unwrap()).unwrap();
        let config = fixture();
        let yaml = config.to_yaml().unwrap();
        std::fs::write(paths.config(), yaml.as_str()).unwrap();
        std::fs::write(paths.quadlet(), render_quadlet(4567, existing).unwrap()).unwrap();
        for path in [paths.config(), paths.quadlet()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let installer = Installer::with_runtime(Prerequisites, HttpProbe::new().unwrap());
        let state = installer
            .inspect(paths, &CancellationToken::new())
            .await
            .unwrap();
        let mut a = Wizard::new(
            Cli::try_parse_from(if nightly {
                vec!["setup", "--nightly"]
            } else {
                vec!["setup"]
            })
            .unwrap(),
        );
        a.load(&state).unwrap();
        assert_eq!(a.host_port, 4567);
        assert_eq!(a.channel, expected);
        assert!(a.config.read_only_test_mode);
        assert_eq!(a.config.to_yaml().unwrap().as_str(), yaml.as_str());
    }
}

#[tokio::test]
async fn saved_forge_destination_change_never_calls_discovery_before_consent() {
    let mut a = app();
    a.config = fixture();
    a.config.jira = None;
    a.config.forges[0].default_project = None;
    let d = MockDiscovery::default();
    let mut p = Script::new(vec![
        Confirm("Configure Jira", false),
        Confirm("Configure existing forge", true),
        Select("Forge provider", 0),
        Text("Forge URL", "https://github-enterprise.invalid/api/v3"),
        Password(true, ""),
        Cancel("Send the saved"),
    ]);
    assert!(
        a.configure(&mut p, &d, &CancellationToken::new())
            .await
            .unwrap_err()
            .is::<Cancelled>()
    );
    assert!(d.forges.borrow().is_empty());
    assert_eq!(a.config.forges[0].base_url, "https://api.github.com");
}

#[tokio::test]
async fn escape_discards_new_forge_at_every_prompt_and_allows_adding_another() {
    for stage in 0..9 {
        let mut a = app();
        let d = MockDiscovery::default();
        let mut steps = vec![
            Confirm("Configure Jira", false),
            Confirm("Add a forge account", true),
        ];
        let prefix = vec![
            Select("Forge provider", 0),
            Enter("Forge URL"),
            Password(false, "discarded-secret"),
            Confirm("Configure advanced repository", true),
            Text("Include repository", "org/*"),
            Enter("Exclude repository"),
        ];
        if stage < 7 {
            let questions = [
                "Forge provider",
                "Forge URL",
                "Token",
                "Configure advanced repository",
                "Include repository",
                "Exclude repository",
                "Default Jira project",
            ];
            steps.extend(prefix.into_iter().take(stage));
            steps.push(Escape(questions[stage]));
        } else if stage == 7 {
            d.forge_failures.borrow_mut().push_back(true);
            steps.extend(prefix.into_iter().take(3));
            steps.push(Escape("Connection failed"));
        } else {
            // Consent is required when retrying with a token for a new destination.
            d.forge_failures.borrow_mut().push_back(true);
            steps.extend(prefix.into_iter().take(3));
            steps.extend([
                Select("Connection failed", 1),
                Select("Forge provider", 0),
                Text("Forge URL", "https://enterprise.invalid/api/v3"),
                Password(true, ""),
                Escape("Send the saved"),
            ]);
        }
        steps.extend([
            Confirm("Add another forge", true),
            Select("Forge provider", 1),
            Enter("Forge URL"),
            Password(false, "kept-secret"),
            Confirm("Configure advanced repository", false),
            Confirm("Add another forge", false),
            Select("AI provider", 0),
        ]);
        steps.extend(runtime());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        assert!(!p.local_cancel);
        assert_eq!(a.config.forges.len(), 1);
        assert_eq!(a.config.forges[0].id, "gitlab-1");
        assert!(!a.summary().join("\n").contains("github-1"));
        assert!(!p.transcript.join("\n").contains("discarded-secret"));
    }
}

#[tokio::test]
async fn escape_existing_forge_keeps_original_config_and_ctrl_c_still_exits() {
    for escape in [true, false] {
        let mut a = app();
        a.config = fixture();
        a.config.jira = None;
        a.config.forges[0].default_project = None;
        let original = serde_json::to_value(&a.config.forges).unwrap();
        let mut steps = vec![
            Confirm("Configure Jira", false),
            Confirm("Configure existing forge", true),
            Select("Forge provider", 1),
            Enter("Forge URL"),
        ];
        steps.push(if escape {
            Escape("Token")
        } else {
            Cancel("Token")
        });
        if escape {
            steps.extend(disabled_tail());
        }
        let mut p = Script::new(steps);
        let d = MockDiscovery::default();
        let result = a.configure(&mut p, &d, &CancellationToken::new()).await;
        if escape {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().is::<Cancelled>());
        }
        p.done();
        assert!(!p.local_cancel);
        assert_eq!(serde_json::to_value(&a.config.forges).unwrap(), original);
        assert!(d.forges.borrow().is_empty());
    }
}

#[tokio::test]
async fn group_menu_supports_confirmed_full_scan_manual_lookup_retry_and_auto_advance() {
    let mut a = app();
    let d = MockDiscovery {
        all_groups: vec![
            Choice::new("developers", "developers"),
            Choice::new("other", "other"),
        ],
        manual_groups: BTreeMap::from([("manual".into(), Choice::new("manual", "manual"))]),
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.truncate(6); // Connection, project picker and board picker.
    steps.extend([
        Menu("Assignee groups"),
        Select("Group actions", 2),
        Confirm("Load ALL accessible Jira groups", false),
        Select("Group actions", 2),
        Confirm("Load ALL accessible Jira groups", true),
        Select("Group actions", 1),
        Text("Exact group name", ""),
        Select("Group actions", 1),
        Text("Exact group name", "missing"),
        Select("Group actions", 1),
        Text("Exact group name", "manual"),
        Select("Group actions", 3),
        Select("Group actions", 0),
        Multi("Assignee groups", vec![0, 1, 2]),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Enter("Regular expression to identify your sprints"),
        Confirm("Configure advanced issue", false),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(
        a.config.jira.unwrap().assignee_groups,
        ["developers", "other", "manual"]
    );
    assert_eq!(
        d.calls
            .borrow()
            .iter()
            .filter(|c| c.as_str() == "all_groups")
            .count(),
        1
    );
    assert_eq!(
        d.calls
            .borrow()
            .iter()
            .filter(|c| c.as_str() == "own_groups")
            .count(),
        2
    );
    assert!(p.transcript.join("\n").contains("Group lookup failed"));
}

#[tokio::test]
async fn saved_groups_outside_memberships_are_verified_preserved_and_fail_closed() {
    for verified in [true, false] {
        let mut a = app();
        a.config = fixture();
        a.config.forges.clear();
        a.config.jira.as_mut().unwrap().projects = vec!["ONE".into()];
        a.config
            .jira
            .as_mut()
            .unwrap()
            .assignee_groups
            .push("saved".into());
        let mut d = MockDiscovery::default();
        if verified {
            d.manual_groups
                .insert("saved".into(), Choice::new("saved", "saved"));
        }
        let mut steps = vec![
            Enter("Configure Jira"),
            Enter("Jira Cloud tenant URL"),
            Enter("Atlassian account email"),
            Password(true, ""),
            Enter("Jira projects"),
            Enter("Jira board"),
            Enter("Assignee groups"),
        ];
        if !verified {
            steps.extend([
                Select("Group actions", 4), // Unverified saved group cannot be continued.
                Select("Group actions", 0),
                Multi("Assignee groups", vec![0]),
                Confirm("Confirm removal of saved assignee groups", false),
                Multi("Assignee groups", vec![0]),
                Confirm("Confirm removal of saved assignee groups", true),
            ]);
        }
        steps.extend([
            Enter("Team field (semantic name):"),
            Enter("Team:"),
            Enter("Regular expression to identify your sprints"),
            Enter("Configure advanced issue"),
        ]);
        steps.extend(disabled_tail());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        assert_eq!(
            a.config
                .jira
                .unwrap()
                .assignee_groups
                .contains(&"saved".into()),
            verified
        );
        assert_eq!(
            d.calls
                .borrow()
                .iter()
                .filter(|c| c.as_str() == "group:saved")
                .count(),
            1
        );
        assert!(!d.calls.borrow().iter().any(|c| c == "all_groups"));
    }
}

#[tokio::test]
async fn failed_or_empty_own_groups_allow_manual_setup_without_automatic_full_scan() {
    for failed in [true, false] {
        let mut a = app();
        let d = MockDiscovery {
            own_groups: Some(vec![]),
            own_groups_fail: failed,
            manual_groups: BTreeMap::from([("manual".into(), Choice::new("manual", "manual"))]),
            ..Default::default()
        };
        let mut steps = jira_new();
        steps.truncate(6);
        steps.extend([
            Select("Group actions", 4),
            Select("Group actions", 1),
            Text("Exact group name", "manual"),
            Select("Group actions", 0),
            Enter("Assignee groups"),
            Enter("Team field (semantic name):"),
            Enter("Team:"),
            Enter("Regular expression to identify your sprints"),
            Enter("Configure advanced issue"),
        ]);
        steps.extend(disabled_tail());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        assert_eq!(a.config.jira.unwrap().assignee_groups, ["manual"]);
        assert!(!d.calls.borrow().iter().any(|c| c == "all_groups"));
    }
}

#[tokio::test]
async fn skipping_optional_advanced_preserves_saved_policies_and_filters_exactly() {
    use pullboard_core::{config::PurposePolicy, model::Workflow};
    let mut a = app();
    a.config = fixture();
    a.config.jira.as_mut().unwrap().projects = vec!["ONE".into()];
    a.config.forges[0].default_project = Some("ONE".into());
    a.config.jira.as_mut().unwrap().defaults.implementation = Some(PurposePolicy {
        issue_type: "Custom task".into(),
        assignee: Some("me".into()),
        workflow: Some(Workflow {
            draft: Some("Draft".into()),
            ready: Some("Ready".into()),
        }),
    });
    let before = serde_json::to_value(&a.config).unwrap();
    let mut steps = vec![
        Confirm("Configure Jira", true),
        Text("Jira Cloud tenant URL", "<default>"),
        Text("Atlassian account email", "<default>"),
        Password(true, ""),
        Enter("Jira projects"),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Text("Regular expression to identify your sprints", "<default>"),
        Confirm("Configure advanced issue", false),
        Confirm("Configure existing forge", true),
        Select("Forge provider", 0),
        Text("Forge URL", "<default>"),
        Password(true, ""),
        Confirm("Configure advanced repository", false),
    ];
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &MockDiscovery::default()).await;
    let after = serde_json::to_value(&a.config).unwrap();
    assert_eq!(before["jira"], after["jira"]);
    assert_eq!(before["forges"], after["forges"]);
}

#[tokio::test]
async fn rerun_enter_keeps_saved_primary_when_multiselect_returns_inventory_order() {
    let mut a = app();
    a.config = fixture();
    a.config.forges.clear();
    a.config.jira.as_mut().unwrap().projects = vec!["TWO".into(), "ONE".into()];
    let before = serde_json::to_value(&a.config.jira).unwrap();
    let mut p = Script::new(vec![
        Enter("Configure Jira"),
        Enter("Jira Cloud tenant URL"),
        Enter("Atlassian account email"),
        Password(true, ""),
        Enter("Jira projects"),
        Enter("Primary Jira project"),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Enter("Regular expression to identify your sprints"),
        Enter("Configure advanced issue"),
        Enter("Add a forge account"),
        Enter("AI provider"),
        Enter("Local host port"),
        Enter("Container channel"),
        Enter("Start in Test mode"),
    ]);
    let d = MockDiscovery {
        projects: vec![Choice::new("ONE", "One"), Choice::new("TWO", "Two")],
        ..Default::default()
    };
    configured(&mut a, &mut p, &d).await;
    assert!(
        p.defaults
            .contains(&("Jira projects:".into(), "[0, 1]".into()))
    );
    assert!(
        p.defaults
            .contains(&("Primary Jira project:".into(), "1".into()))
    );
    assert_eq!(a.config.jira.as_ref().unwrap().projects, ["TWO", "ONE"]);
    assert_eq!(serde_json::to_value(&a.config.jira).unwrap(), before);
}

#[tokio::test]
async fn empty_or_failed_recents_allow_manual_disambiguation_and_reuse_verified_entries() {
    for failed in [false, true] {
        let mut a = app();
        let d = MockDiscovery {
            empty_projects: true,
            empty_boards: true,
            manual_projects: BTreeMap::from([(
                "Shared project".into(),
                vec![
                    Choice::new("ONE", "ONE Shared project"),
                    Choice::new("TWO", "TWO Shared project"),
                ],
            )]),
            manual_boards: BTreeMap::from([(
                "https://first.atlassian.net/jira/software/projects/TWO/boards/88".into(),
                Choice::new("88", "88 Manual board"),
            )]),
            ..Default::default()
        };
        if failed {
            d.failures.borrow_mut().push_back(true);
            d.board_failures.borrow_mut().push_back(true);
        }
        let mut steps = jira_new();
        steps.truncate(4);
        steps.extend([
            Select("Project actions", 4), // Cannot continue with an empty selection.
            Select("Project actions", 1),
            Text("Project key", ""), // Back without a read.
            Select("Project actions", 1),
            Text("Project key", "Shared project"),
            Select("Several projects have this exact name", 1),
            Select("Project actions", 1),
            Text("Project key", "TWO"), // Known row reused.
            Select("Project actions", 4),
            Select("Board actions", 4),
            Select("Board actions", 1),
            Text("Board ID", ""),
            Select("Board actions", 1),
            Text(
                "Board ID",
                "https://first.atlassian.net/jira/software/projects/TWO/boards/88",
            ),
            Select("Board actions", 1),
            Text("Board ID", "88"), // Known row reused.
            Select("Board actions", 4),
            Enter("Assignee groups"),
            Enter("Team field (semantic name):"),
            Enter("Team:"),
            Text("Regular expression to identify your sprints", "<default>"),
            Confirm("Configure advanced issue", false),
        ]);
        steps.extend(disabled_tail());
        let mut p = Script::new(steps);
        configured(&mut a, &mut p, &d).await;
        let jira = a.config.jira.unwrap();
        assert_eq!(jira.projects, ["TWO"]);
        assert_eq!(jira.board_id, 88);
        let calls = d.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("project:"))
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("board:"))
                .count(),
            1
        );
        assert_eq!(
            &calls[calls.len() - 3..],
            ["settings", "metadata", "board_teams:88"]
        );
        assert!(
            !calls
                .iter()
                .any(|call| call.starts_with("all_projects") || call.starts_with("project_boards"))
        );
        let log = p.transcript.join("\n");
        assert!(log.contains("non-exhaustive"));
        assert!(log.contains("Manual project entry is still available"));
        if failed {
            assert!(log.contains("HTTP 403") && log.contains("HTTP 404"));
        }
        let offered = p
            .options
            .iter()
            .find(|(q, _)| q.starts_with("Several projects have this exact name"))
            .unwrap();
        assert_eq!(
            offered.1.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["ONE", "TWO"]
        );
    }
}

#[tokio::test]
async fn broad_lookups_require_separate_warnings_and_explicit_confirmation_and_stay_scoped() {
    let mut a = app();
    let d = MockDiscovery {
        empty_projects: true,
        empty_boards: true,
        all_projects: vec![Choice::new("TWO", "Two"), Choice::new("ONE", "One")],
        project_boards: vec![
            Choice::new("9", "9 Scoped board"),
            Choice::new("10", "10 Scoped board"),
        ],
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.truncate(4);
    steps.extend([
        Select("Project actions", 2),
        Confirm("Load ALL accessible Jira projects", false),
        Select("Project actions", 2),
        Confirm("Load ALL accessible Jira projects", true),
        Select("Project actions", 0),
        Multi("Jira projects", vec![0, 1]),
        Select("Primary Jira project", 1),
        Select("Board actions", 2),
        Confirm("Load ALL boards for selected projects", false),
        Select("Board actions", 2),
        Confirm("Load ALL boards for selected projects", true),
        Select("Board actions", 0),
        Select("Jira board", 1),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Text("Regular expression to identify your sprints", "<default>"),
        Confirm("Configure advanced issue", false),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(
        *d.calls.borrow(),
        [
            "recents",
            "all_projects",
            "boards",
            "project_boards:ONE,TWO",
            "own_groups",
            "settings",
            "metadata",
            "board_teams:10"
        ]
    );
    assert_eq!(a.config.jira.as_ref().unwrap().projects, ["ONE", "TWO"]);
    assert_eq!(a.config.jira.as_ref().unwrap().board_id, 10);
    let log = p.transcript.join("\n");
    assert!(log.contains("Warning: loading all accessible projects"));
    assert!(log.contains("Warning: loading every board for selected projects [ONE, TWO]"));
    assert_eq!(log.matches("Jira API limits").count(), 4);
    for (question, default) in &p.defaults {
        if question.starts_with("Load ALL") {
            assert_eq!(default, "false");
        }
    }
}

#[tokio::test]
async fn failed_broad_and_manual_reads_keep_the_manager_usable_and_explain_the_cause() {
    let mut a = app();
    let d = MockDiscovery {
        broad_failure: true,
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.truncate(4);
    steps.extend([
        Menu("Jira projects"),
        Select("Project actions", 2),
        Confirm("Load ALL accessible Jira projects", true),
        Select("Project actions", 1),
        Text("Project key", "MISSING"),
        Select("Project actions", 0),
        Enter("Jira projects"),
        Menu("Jira board"),
        Select("Board actions", 2),
        Confirm("Load ALL boards for selected projects", true),
        Select("Board actions", 1),
        Text("Board ID", "999"),
        Select("Board actions", 0),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Text("Regular expression to identify your sprints", "<default>"),
        Confirm("Configure advanced issue", false),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    let log = p.transcript.join("\n");
    assert!(log.contains("max_pages 100 reached"));
    assert!(log.contains("project not found or not permitted (HTTP 404)"));
    assert!(log.contains("board not found or not permitted (HTTP 404)"));
    assert_eq!(
        &d.calls.borrow()[d.calls.borrow().len() - 3..],
        ["settings", "metadata", "board_teams:7"]
    );
}

#[tokio::test]
async fn saved_selections_absent_from_recents_are_verified_once_and_preserved_in_order() {
    let mut a = app();
    a.config = fixture();
    a.config.forges.clear();
    a.config.jira.as_mut().unwrap().projects = vec!["THREE".into(), "TWO".into(), "ONE".into()];
    a.config.jira.as_mut().unwrap().board_id = 99;
    let before = serde_json::to_value(&a.config.jira).unwrap();
    let d = MockDiscovery {
        manual_projects: BTreeMap::from([
            ("THREE".into(), vec![Choice::new("THREE", "Three")]),
            ("TWO".into(), vec![Choice::new("TWO", "Two")]),
        ]),
        manual_boards: BTreeMap::from([("99".into(), Choice::new("99", "Saved board"))]),
        ..Default::default()
    };
    let mut steps = vec![
        Enter("Configure Jira"),
        Enter("Jira Cloud tenant URL"),
        Enter("Atlassian account email"),
        Password(true, ""),
        Menu("Jira projects"),
        Select("Project actions", 3), // Successful verification is reused even on retry.
        Select("Project actions", 0),
        Enter("Jira projects"),
        Enter("Primary Jira project"),
        Menu("Jira board"),
        Select("Board actions", 3),
        Select("Board actions", 0),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Enter("Regular expression to identify your sprints"),
        Enter("Configure advanced issue"),
    ];
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(serde_json::to_value(&a.config.jira).unwrap(), before);
    let calls = d.calls.borrow();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "project:THREE")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "project:TWO")
            .count(),
        1
    );
    assert!(!calls.iter().any(|call| call == "project:ONE"));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "board:99")
            .count(),
        1
    );
    assert_eq!(a.config.jira.unwrap().projects, ["THREE", "TWO", "ONE"]);
}

#[tokio::test]
async fn unverified_saved_projects_cannot_be_saved_or_silently_removed_and_removal_cleans_links() {
    let mut a = app();
    a.config = fixture();
    a.config
        .jira
        .as_mut()
        .unwrap()
        .project_overrides
        .insert("TWO".into(), ProjectOverride::default());
    let d = MockDiscovery::default();
    let mut steps = vec![
        Enter("Configure Jira"),
        Enter("Jira Cloud tenant URL"),
        Enter("Atlassian account email"),
        Password(true, ""),
        Menu("Jira projects"),
        Enter("Project actions"), // TWO is absent and its direct verification failed.
        Select("Project actions", 0),
        Multi("Jira projects", vec![0]),
        Confirm("Confirm project removal", false),
        Multi("Jira projects", vec![0]),
        Confirm("Confirm project removal", true),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Enter("Regular expression to identify your sprints"),
        Enter("Configure advanced issue"),
        Confirm("Configure existing forge", true),
        Select("Forge provider", 0),
        Enter("Forge URL"),
        Password(true, ""),
        Confirm("Configure advanced repository", false),
    ];
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    assert_eq!(a.config.jira.as_ref().unwrap().projects, ["ONE"]);
    assert!(
        !a.config
            .jira
            .as_ref()
            .unwrap()
            .project_overrides
            .contains_key("TWO")
    );
    assert!(a.config.forges[0].default_project.is_none());
    assert!(
        p.transcript
            .join("\n")
            .contains("Some saved projects are not verified")
    );
    let offered = p
        .options
        .iter()
        .find(|(q, _)| q == "Jira projects:")
        .unwrap();
    assert!(offered.1[1].label.contains("NOT verified"));
}

#[tokio::test]
async fn manual_failures_redact_actual_literal_encoded_and_basic_secrets_without_hiding_safe_status()
 {
    let mut a = app();
    let d = MockDiscovery {
        manual_error: Some(
            "Jira HTTP 429; Retry-After 30 seconds; new-jira-secret %6E%65%77%2D%6A%69%72%61%2D%73%65%63%72%65%74 aHVtYW5AZXhhbXBsZS5pbnZhbGlkOm5ldy1qaXJhLXNlY3JldA== https://unsafe.invalid/private?q=secret \u{1b}[31m",
        ),
        ..Default::default()
    };
    let mut steps = jira_new();
    steps.truncate(4);
    steps.extend([
        Menu("Jira projects"),
        Select("Project actions", 1),
        Text("Project key", "MISSING"),
        Select("Project actions", 0),
        Enter("Jira projects"),
        Menu("Jira board"),
        Select("Board actions", 1),
        Text("Board ID", "999"),
        Select("Board actions", 0),
        Enter("Jira board"),
        Enter("Assignee groups"),
        Enter("Team field (semantic name):"),
        Enter("Team:"),
        Enter("Regular expression to identify your sprints"),
        Enter("Configure advanced issue"),
    ]);
    steps.extend(disabled_tail());
    let mut p = Script::new(steps);
    configured(&mut a, &mut p, &d).await;
    let log = p.transcript.join("\n");
    assert!(log.contains("HTTP 429; Retry-After 30 seconds"));
    for unsafe_text in [
        "new-jira-secret",
        "%6E%65%77",
        "aHVtYW5AZXhhbXBsZS5pbnZhbGlkOm5ldy1qaXJhLXNlY3JldA==",
        "https://unsafe",
        "\u{1b}",
    ] {
        assert!(!log.contains(unsafe_text));
    }
}

#[tokio::test]
async fn cancelling_manual_or_manager_prompts_preserves_the_pending_configuration_and_defers_settings()
 {
    for question in [
        "Project actions",
        "Project key",
        "Board actions",
        "Board ID",
    ] {
        let mut a = app();
        a.config = fixture();
        a.config.forges.clear();
        a.config.jira.as_mut().unwrap().projects = vec!["ONE".into()];
        let before = a.config.to_yaml().unwrap();
        let d = MockDiscovery::default();
        let mut steps = vec![
            Enter("Configure Jira"),
            Enter("Jira Cloud tenant URL"),
            Enter("Atlassian account email"),
            Password(true, ""),
        ];
        if question.starts_with("Board") {
            steps.push(Enter("Jira projects"));
            steps.push(Menu("Jira board"));
        } else {
            steps.push(Menu("Jira projects"));
        }
        if question == "Project key" {
            steps.push(Select("Project actions", 1));
        }
        if question == "Board ID" {
            steps.push(Select("Board actions", 1));
        }
        steps.push(Cancel(question));
        let mut p = Script::new(steps);
        assert!(
            a.configure(&mut p, &d, &CancellationToken::new())
                .await
                .unwrap_err()
                .is::<Cancelled>()
        );
        p.done();
        assert_eq!(a.config.to_yaml().unwrap().as_str(), before.as_str());
        assert!(
            !d.calls
                .borrow()
                .iter()
                .any(|call| call == "settings" || call == "metadata")
        );
    }
}

/// Exact queued-token startup boundary from the review, without input or sleeps.
#[tokio::test]
#[ignore = "prompt-only startup race fixture driven by inline_smoke.py"]
async fn terminal_startup_cancel_fixture() {
    assert!(std::env::var_os("PULLBOARD_PTY_FIXTURE").is_some());
    let cancel = CancellationToken::new();
    let mut p = pullboard_setup::prompt::TerminalPrompts::new(true, cancel.clone());
    tokio::spawn(async move {
        cancel.cancel();
    });
    let result = p.confirm("Queued startup cancellation only?", false).await;
    assert!(result.unwrap_err().is::<Cancelled>());
}

/// Only a PTY driver opts into this fixture. No installer or upstream network.
#[tokio::test]
#[ignore = "interactive fixture driven by inline_smoke.py"]
async fn terminal_fixture() {
    assert!(
        std::env::var_os("PULLBOARD_PTY_FIXTURE").is_some(),
        "run inline_smoke.py instead"
    );
    let cancel = CancellationToken::new();
    let signals = pullboard_setup::prompt::cancellation_signals(cancel.clone()).unwrap();
    let mut p = pullboard_setup::prompt::TerminalPrompts::new(
        std::env::var_os("NO_COLOR").is_some(),
        cancel.clone(),
    );
    p.say("Mock prerequisites available. No installation/network is possible in this fixture.");
    let mut d = MockDiscovery {
        projects: vec![
            Choice::new("ONE", "ONE Mock project"),
            Choice::new("TWO", "TWO Mock project"),
        ],
        ..Default::default()
    };
    if std::env::var_os("PULLBOARD_PTY_MANUAL").is_some() {
        d.empty_projects = true;
        d.empty_boards = true;
        d.failures.borrow_mut().push_back(true);
        d.board_failures.borrow_mut().push_back(true);
        d.manual_projects
            .insert("TWO".into(), vec![Choice::new("TWO", "TWO Manual project")]);
        d.manual_boards
            .insert("88".into(), Choice::new("88", "88 Manual board"));
    }
    let mut a = app();
    let result = async {
        a.configure(&mut p, &d, &cancel).await?;
        a.authorize(&mut p).await?;
        Result::<()>::Ok(())
    }
    .await;
    if let Err(e) = result {
        assert!(e.is::<Cancelled>());
        p.say("setup cancelled; no files changed");
    }
    signals.abort();
}
