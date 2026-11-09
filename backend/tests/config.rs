mod common;
use common::*;
use pullboard::config::{Config, Secret};

#[tokio::test]
async fn all_sanitized_examples_validate_without_resolving_placeholder_credentials() {
    for file in ["pullboard.yaml", "container.yaml", "example.yaml"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../config")
            .join(file);
        Config::load(&path).await.unwrap();
    }
}
#[test]
fn separate_instance_ids_are_supported_and_duplicate_ids_rejected() {
    let mut c = config();
    let first = forge("gitlab", "https://work.example");
    let mut second = forge("gitlab", "https://community.example");
    second.id = "another-instance".into();
    c.forges = vec![first, second];
    c.validate().unwrap();
    c.forges[1].id = c.forges[0].id.clone();
    assert!(c.validate().is_err());
}

#[test]
fn strict_policy_only_schema_and_forge_name_fallback() {
    let minimal = "base_url: https://jira.example\nemail: me@example.invalid\ntoken: {value: placeholder}\nprojects: [DEV]\nboard_id: 42\nteam: Platform\nsprint_name_pattern: '%YEAR-Platform-S%NUMBER'\n";
    for removed in [
        "account_id: old",
        "display_name: old",
        "fields: {}",
        "teams: []",
        "workflow: {}",
        "sprints: []",
    ] {
        assert!(
            serde_yaml::from_str::<pullboard::config::JiraConfig>(&format!("{minimal}{removed}\n"))
                .is_err()
        );
    }
    for required in [
        "board_id: 42\n",
        "team: Platform\n",
        "sprint_name_pattern: '%YEAR-Platform-S%NUMBER'\n",
    ] {
        assert!(
            serde_yaml::from_str::<pullboard::config::JiraConfig>(&minimal.replace(required, ""))
                .is_err()
        );
    }
    assert!(
        serde_yaml::from_str::<pullboard::config::JiraConfig>(&minimal.replace(
            "projects: [DEV]",
            "projects: [{key: DEV, name: Development}]"
        ))
        .is_err()
    );
    let f: pullboard::config::ForgeConfig = serde_yaml::from_str(
        "id: stable\nkind: github\nbase_url: https://api.github.com\ntoken: {value: placeholder}",
    )
    .unwrap();
    assert_eq!(f.name(), "stable");
    assert!(serde_yaml::from_str::<pullboard::config::ForgeConfig>("id: stable\nkind: github\nbase_url: https://api.github.com\ntoken: {value: placeholder}\naccount: {username: old}").is_err());
    let mut c = config();
    c.jira = Some(
        serde_yaml::from_str(&format!("{minimal}assignee_groups: [one, two, one]\n")).unwrap(),
    );
    c.validate().unwrap();
    assert_eq!(c.jira.unwrap().assignee_groups, ["one", "two"]);
    for invalid in [
        "board_id: 0",
        "team: ' '",
        "team_field_name: customfield_42",
        "team_field_name: '42'",
        "assignee_groups: [' ']",
    ] {
        let yaml = if invalid.starts_with("board_id") {
            minimal.replace("board_id: 42", invalid)
        } else if invalid.starts_with("team:") {
            minimal.replace("team: Platform", invalid)
        } else {
            format!("{minimal}{invalid}\n")
        };
        let mut c = config();
        c.jira = Some(serde_yaml::from_str(&yaml).unwrap());
        assert!(c.validate().is_err(), "invalid policy was accepted");
    }
}

#[test]
fn sprint_pattern_is_anchored_literal_and_ascii_not_current_year() {
    let mut c = config();
    let mut blank = jira("https://jira.example");
    blank.sprint_name_pattern = " \t\n".into();
    assert!(blank.sprint_pattern().is_err());
    c.jira = Some(blank);
    assert!(c.validate().is_err());
    let mut j = jira("https://jira.example");
    j.sprint_name_pattern = "%YEAR-X.+-S%NUMBER".into();
    let p = j.sprint_pattern().unwrap();
    for name in ["1999-X.+-S1", "2027-X.+-S002"] {
        assert!(p.is_match(name));
    }
    for name in [
        "2026-Xaa-S1",
        "x2026-X.+-S1",
        "2026-X.+-S1x",
        "2026-X.+-S",
        "２０２６-X.+-S1",
        "2026-X.+-S１",
        "2026-X.+-S1\n",
    ] {
        assert!(!p.is_match(name));
    }
    for bad in ["%", "%MONTH", "%YEARS", "%NUMBERED", "%year"] {
        j.sprint_name_pattern = bad.into();
        assert!(j.sprint_pattern().is_err());
    }
    j.sprint_name_pattern = "Literal (not regex)".into();
    assert!(j.sprint_pattern().unwrap().is_match("Literal (not regex)"));
}

#[test]
fn purpose_policies_replace_only_selected_purpose_and_inherit_workflow() {
    use pullboard::model::Purpose;
    let mut j = jira("https://jira.example");
    assert!(j.policy("DEV", Purpose::Implementation).assignee.is_none());
    assert!(j.policy("DEV", Purpose::Review).assignee.is_none());
    j.defaults = serde_yaml::from_str("implementation: {issue_type: Story, assignee: me, workflow: {draft: Coding, ready: Review}}\nreview: {issue_type: Task, assignee: null}").unwrap();
    j.project_overrides = serde_yaml::from_str(
        "DEV: {defaults: {implementation: {issue_type: Task, assignee: null}}}",
    )
    .unwrap();
    let policy = j.policy("DEV", Purpose::Implementation);
    assert_eq!(policy.issue_type, "Task");
    assert!(policy.assignee.is_none());
    assert_eq!(policy.workflow.unwrap().draft.as_deref(), Some("Coding"));
    for yaml in [
        "implementation: null",
        "review: {issue_type: Task, assignee: null, workflow: null}",
        "implementation: {issue_type: Task}",
        "review: {issue_type: Task, assignee: null, workflow: {draft: null}}",
    ] {
        assert!(serde_yaml::from_str::<pullboard::config::PurposeDefaults>(yaml).is_err());
    }
    for yaml in [
        "review: {issue_type: Task, assignee: me}",
        "implementation: {issue_type: Task, assignee: fixed-account}",
        "implementation: {issue_type: ' ', assignee: null}",
    ] {
        let mut c = config();
        let mut j = jira("https://jira.example");
        j.defaults = serde_yaml::from_str(yaml).unwrap();
        c.jira = Some(j);
        assert!(c.validate().is_err());
    }
}
#[test]
fn invalid_keyring_refs_and_custom_fields_are_rejected_and_config_debug_redacts() {
    let mut c = config();
    c.forges = vec![forge("github", "https://api.github.com")];
    c.forges[0].token =
        serde_yaml::from_str::<Secret>("keyring: {service: '', account: me}").unwrap();
    assert!(c.validate().is_err());
    c.forges[0].token = Secret::Literal {
        value: "never-print-this".into(),
    };
    assert!(!format!("{c:?}").contains("never-print-this"));
    c.jira = Some(jira("https://jira.example"));
    c.jira.as_mut().unwrap().team_field_name = "customfield_123".into();
    assert!(c.validate().is_err());
}
#[test]
fn invalid_bounds_and_base_url_queries_are_rejected() {
    let mut c = config();
    c.refresh_interval_seconds = u64::MAX;
    assert!(c.validate().is_err());
    c.refresh_interval_seconds = 3600;
    c.forges = vec![forge("github", "https://api.github.com?token=not-allowed")];
    assert!(c.validate().is_err());
    c.forges[0].base_url = "https://api.github.com".into();
    c.upstream.page_size = 0;
    assert!(c.validate().is_err());
}

#[test]
fn parallel_upstream_defaults_and_inclusive_bounds_are_validated() {
    let c: Config = serde_yaml::from_str("upstream: {}\n").unwrap();
    assert_eq!(c.upstream.max_concurrent_requests, 4);
    assert_eq!(c.upstream.max_concurrent_items, 4);
    assert_eq!(c.upstream.github_batch_size, 5);
    for (key, max) in [
        ("max_concurrent_requests", 16),
        ("max_concurrent_items", 32),
        ("github_batch_size", 10),
    ] {
        for value in [1, max] {
            let c: Config = serde_yaml::from_str(&format!("upstream: {{{key}: {value}}}")).unwrap();
            c.validate().unwrap();
        }
        for value in [0, max + 1] {
            let c: Config = serde_yaml::from_str(&format!("upstream: {{{key}: {value}}}")).unwrap();
            assert!(c.validate().is_err(), "accepted out-of-bounds {key}");
        }
        for value in ["-1", "null", "1.5", "'4'"] {
            assert!(
                serde_yaml::from_str::<Config>(&format!("upstream: {{{key}: {value}}}")).is_err()
            );
        }
    }
}
#[tokio::test]
async fn missing_keyring_credential_has_explicit_sanitized_error_and_no_plaintext_fallback() {
    // Exercise keyring's published mock backend without needing an unlocked desktop.
    // Real native/container Secret Service interoperability requires deployment testing.
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let secret: Secret =
        serde_yaml::from_str("keyring: {service: missing-test-service, account: private-account}")
            .unwrap();
    let error = secret.resolve().await.unwrap_err().to_string();
    assert!(error.contains("keyring credential unavailable"));
    assert!(!error.contains("private-account"));
}
