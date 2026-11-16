use clap::{Args, Parser};
use pullboard_core::config::{SprintPattern, project_key};

/// Tokens are entered interactively only; no credential arguments or environment inputs.
#[derive(Parser, Debug)]
#[command(
    name = "pullboard-setup",
    version,
    about = "Configure and install PullBoard with rootless Podman (Linux x86-64)"
)]
pub struct Cli {
    /// Prefill the editable Jira Cloud tenant URL; does not authorize a network request.
    #[arg(long, value_parser = jira_cloud_url, conflicts_with = "url")]
    pub jira_cloud_url: Option<String>,
    #[command(flatten)]
    pub jira: JiraArgs,
    /// Preselect the Nightly container image; editable in Runtime.
    #[arg(long)]
    pub nightly: bool,
    /// Use terminal defaults without color (also honors NO_COLOR).
    #[arg(long)]
    pub no_color: bool,
}

/// Nonsecret team settings. Supplied values bypass pickers, never verification.
#[derive(Args, Clone, Debug, Default)]
pub struct JiraArgs {
    /// Jira Cloud HTTPS tenant root; skips the URL prompt (unlike --jira-cloud-url).
    #[arg(long = "jira-url", value_parser = jira_cloud_url)]
    pub url: Option<String>,
    /// Comma-separated project keys, primary first (1..100 unique projects).
    #[arg(long = "jira-projects", value_parser = projects)]
    pub projects: Option<JiraList>,
    /// Repeat for ordered board IDs (My board first); every board is verified.
    #[arg(long = "jira-board-id", value_parser = board_id)]
    pub board_ids: Vec<u64>,
    /// Comma-separated exact assignee-group names; at least one is required.
    #[arg(long = "jira-groups", value_parser = groups)]
    pub groups: Option<JiraList>,
    /// Exact semantic team field name, not a customfield ID.
    #[arg(long = "jira-team-field", value_parser = team_field)]
    pub team_field: Option<String>,
    /// Exact team option name, verified against project/type create metadata.
    #[arg(long = "jira-team", value_parser = nonblank)]
    pub team: Option<String>,
    /// Repeat for ordered bounded Rust regexes (My pattern first); never comma-split.
    #[arg(long = "jira-sprint-name-pattern", value_parser = sprint_pattern)]
    pub sprint_name_patterns: Vec<SprintPattern>,
}

impl JiraArgs {
    pub fn any(&self) -> bool {
        self.url.is_some()
            || self.projects.is_some()
            || !self.board_ids.is_empty()
            || self.groups.is_some()
            || self.team_field.is_some()
            || self.team.is_some()
            || !self.sprint_name_patterns.is_empty()
    }

    pub fn complete(&self) -> bool {
        self.url.is_some()
            && self.projects.is_some()
            && !self.board_ids.is_empty()
            && self.groups.is_some()
            && self.team_field.is_some()
            && self.team.is_some()
            && !self.sprint_name_patterns.is_empty()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.board_ids.is_empty() {
            pullboard_core::config::validate_board_ids(&self.board_ids)?;
        }
        if !self.sprint_name_patterns.is_empty() {
            pullboard_core::config::validate_sprint_patterns(&self.sprint_name_patterns)?;
        }
        Ok(())
    }
}

/// Trimmed, nonempty, deduplicated names in first-occurrence order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JiraList(pub Vec<String>);

fn nonblank(value: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err("use a nonblank value without control characters".into());
    }
    Ok(value.trim().to_owned())
}

fn groups(value: &str) -> Result<JiraList, String> {
    let mut names = Vec::new();
    for value in value.split(',') {
        let name = nonblank(value)?;
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(JiraList(names))
}

fn projects(value: &str) -> Result<JiraList, String> {
    let names = groups(value)?;
    if names.0.len() > 100 || !names.0.iter().all(|name| project_key(name)) {
        return Err("use 1..100 valid Jira project keys".into());
    }
    Ok(names)
}

fn board_id(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0 && value.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| "use a positive numeric Jira board ID".into())
}

pub(crate) fn semantic_field(s: &str) -> bool {
    !s.trim().is_empty()
        && !s.to_lowercase().starts_with("customfield")
        && !s.bytes().all(|b| b.is_ascii_digit())
        && !s
            .strip_prefix("field")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

fn team_field(value: &str) -> Result<String, String> {
    let name = nonblank(value)?;
    if !semantic_field(&name) {
        return Err("use a semantic field name, not a raw field ID".into());
    }
    Ok(name)
}

fn sprint_pattern(value: &str) -> Result<SprintPattern, String> {
    if value.chars().any(char::is_control) {
        return Err("sprint pattern must not contain control characters".into());
    }
    SprintPattern::new(value).map_err(|_| "use a bounded Rust regex (max 4096 UTF-8 bytes)".into())
}

/// Only direct HTTPS Atlassian tenant roots, never the scoped-token API gateway.
pub fn jira_cloud_url(value: &str) -> Result<String, String> {
    let error = || {
        "use an HTTPS Jira Cloud tenant root, without credentials, path, query or fragment"
            .to_owned()
    };
    if value.contains(['\\', '%'])
        || value.chars().any(char::is_whitespace)
        || value
            .strip_prefix("https://")
            .is_none_or(|s| s.trim_end_matches('/').contains(['/', ':']))
    {
        return Err(error());
    }
    let u = url::Url::parse(value).map_err(|_| error())?;
    let host = u.host_str().ok_or_else(error)?;
    let tenant = host.strip_suffix(".atlassian.net").unwrap_or_default();
    if u.scheme() != "https"
        || tenant.is_empty()
        || tenant.contains('.')
        || !tenant
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(error());
    }
    Ok(u.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeatable_board_and_pattern_flags_preserve_order_and_never_split_regex_commas() {
        let args = Cli::try_parse_from([
            "setup",
            "--jira-board-id=14585",
            "--jira-board-id",
            "11235",
            "--jira-sprint-name-pattern=SSSD-IDENTITY",
            "--jira-sprint-name-pattern",
            "^CORE[0-9]{1,3}$",
        ])
        .unwrap()
        .jira;
        args.validate().unwrap();
        assert_eq!(args.board_ids, [14585, 11235]);
        assert_eq!(
            args.sprint_name_patterns
                .iter()
                .map(SprintPattern::as_str)
                .collect::<Vec<_>>(),
            ["SSSD-IDENTITY", "^CORE[0-9]{1,3}$"]
        );
        for count in [100, 101] {
            let mut argv = vec!["setup".to_owned()];
            for id in 1..=count {
                argv.push(format!("--jira-board-id={id}"));
                argv.push(format!("--jira-sprint-name-pattern=Team{id}"));
            }
            let args = Cli::try_parse_from(argv).unwrap().jira;
            assert_eq!(args.validate().is_ok(), count == 100);
        }
    }
    #[test]
    fn both_clap_forms_and_display_preferences() {
        for args in [
            vec!["setup", "--jira-cloud-url=https://example.atlassian.net"],
            vec![
                "setup",
                "--jira-cloud-url",
                "https://example.atlassian.net/",
            ],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap().jira_cloud_url.as_deref(),
                Some("https://example.atlassian.net")
            );
        }
        assert!(Cli::try_parse_from(["setup", "--token", "secret"]).is_err());
        let c = Cli::try_parse_from(["setup", "--nightly", "--no-color"]).unwrap();
        assert!(c.nightly && c.no_color);
    }
    #[test]
    fn rejects_gateway_and_destination_ambiguity() {
        for s in [
            "http://example.atlassian.net",
            "https://api.atlassian.com/ex/jira/abc",
            "https://user@example.atlassian.net",
            "https://example.atlassian.net/?x=1",
            "https://example.atlassian.net/#x",
            "https://example.atlassian.net/rest/api",
            "https://example.atlassian.net/a/..",
            "https://example.atlassian.net\\@evil.test",
            "https://example.atlassian.net:8443",
            "https://example.atlassian.net:443",
            "https://exam\tple.atlassian.net",
        ] {
            assert!(jira_cloud_url(s).is_err(), "{s}");
        }
    }

    const TEAM_FLAGS: [(&str, &str); 7] = [
        ("--jira-url", "https://example.atlassian.net/"),
        ("--jira-projects", " TWO, ONE, TWO "),
        ("--jira-board-id", "42"),
        ("--jira-groups", " developers, platform team, developers "),
        ("--jira-team-field", "AssignedTeam"),
        ("--jira-team", "Team A"),
        ("--jira-sprint-name-pattern", "(?i)^Team A "),
    ];

    #[test]
    fn team_flags_accept_both_forms_and_preserve_order_and_regex_whitespace() {
        for equals in [false, true] {
            let mut argv = vec!["setup".to_owned()];
            for (flag, value) in TEAM_FLAGS {
                if equals {
                    argv.push(format!("{flag}={value}"));
                } else {
                    argv.extend([flag.into(), value.into()]);
                }
            }
            let args = Cli::try_parse_from(argv).unwrap().jira;
            assert!(args.any() && args.complete());
            assert_eq!(args.url.as_deref(), Some("https://example.atlassian.net"));
            assert_eq!(args.projects.unwrap().0, ["TWO", "ONE"]);
            assert_eq!(args.groups.unwrap().0, ["developers", "platform team"]);
            assert_eq!(args.board_ids, [42]);
            assert_eq!(args.team_field.as_deref(), Some("AssignedTeam"));
            assert_eq!(args.team.as_deref(), Some("Team A"));
            assert_eq!(args.sprint_name_patterns[0].as_str(), "(?i)^Team A ");
        }
        let args = Cli::try_parse_from(["setup", "--jira-team=Team A"])
            .unwrap()
            .jira;
        assert!(args.any() && !args.complete());
    }

    #[test]
    fn rejects_missing_empty_and_repeated_team_flags_and_conflicting_url_modes() {
        for (flag, value) in TEAM_FLAGS {
            for argv in [vec!["setup", flag], vec!["setup", flag, ""]] {
                assert!(Cli::try_parse_from(argv).is_err(), "{flag}");
            }
            let repeated = Cli::try_parse_from(["setup", flag, value, flag, value]);
            if matches!(flag, "--jira-board-id" | "--jira-sprint-name-pattern") {
                assert!(repeated.unwrap().jira.validate().is_err());
            } else {
                assert!(repeated.is_err());
            }
        }
        assert!(
            Cli::try_parse_from([
                "setup",
                "--jira-url=https://example.atlassian.net",
                "--jira-cloud-url=https://example.atlassian.net"
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_team_setting_shapes_without_network_access() {
        for (flag, values) in [
            (
                "--jira-url",
                vec![
                    "http://example.atlassian.net",
                    "https://example.com",
                    "https://example.atlassian.net/api",
                ],
            ),
            (
                "--jira-projects",
                vec!["ONE,,TWO", ",ONE", "ONE,", "one", "ONE,Not a key"],
            ),
            (
                "--jira-groups",
                vec![",developers", "developers,", "a,,b", "   ", "a\nb"],
            ),
            (
                "--jira-board-id",
                vec!["0", "-1", "+1", "one", "18446744073709551616"],
            ),
            (
                "--jira-team-field",
                vec![" ", "customfield_123", "field123", "123", "Team\n"],
            ),
            ("--jira-team", vec![" ", "Team\n", "\u{1b}[31mTeam"]),
            (
                "--jira-sprint-name-pattern",
                vec![" ", "[", "(?=Team)", "a\nb"],
            ),
        ] {
            for value in values {
                assert!(
                    Cli::try_parse_from(["setup", flag, value]).is_err(),
                    "{flag}"
                );
            }
        }
        let too_many = (0..101)
            .map(|n| format!("P{n}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(Cli::try_parse_from(["setup", "--jira-projects", &too_many]).is_err());
        assert!(
            Cli::try_parse_from(["setup", "--jira-sprint-name-pattern", &"a".repeat(4097)])
                .is_err()
        );
    }
}
