use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pullboard-setup"))
}

#[test]
fn help_and_version_do_not_inspect_or_install() {
    let help = binary().arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for flag in [
        "--jira-cloud-url",
        "--jira-url",
        "--jira-projects",
        "--jira-board-id",
        "--jira-groups",
        "--jira-team-field",
        "--jira-team",
        "--jira-sprint-name-pattern",
        "--nightly",
        "--no-color",
    ] {
        assert!(help.contains(flag));
    }
    assert!(!help.contains("--token"));
    assert!(binary().arg("--version").output().unwrap().status.success());
}

#[test]
fn rejects_noninteractive_execution_before_reading_configuration() {
    let home = tempfile::tempdir().unwrap();
    let output = binary()
        .env("HOME", home.path())
        .env("NO_COLOR", "1")
        .arg("--jira-cloud-url=https://example.atlassian.net")
        .arg("--nightly")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert_eq!(home.path().read_dir().unwrap().count(), 0);
}

#[test]
fn invalid_destinations_and_credentials_arguments_fail_before_tui() {
    for args in [
        vec!["--jira-cloud-url", "https://api.atlassian.com/ex/jira/id"],
        vec!["--token", "synthetic"],
    ] {
        let output = binary().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(!output.stdout.starts_with(b"\x1b"));
    }
}

#[test]
fn rejected_argument_values_never_echo_accidental_credentials() {
    const SECRET: &str = "SYNTHETIC_REVIEW_SECRET";
    for url in [
        format!("https://user:{SECRET}@example.atlassian.net"),
        format!("https://example.atlassian.net/?token={SECRET}"),
        format!("https://example.atlassian.net/#{SECRET}"),
    ] {
        for args in [
            vec![format!("--jira-cloud-url={url}")],
            vec!["--jira-cloud-url".into(), url.clone()],
        ] {
            assert_sanitized_rejection(&args, SECRET);
        }
    }
    for args in [
        vec![format!("--token={SECRET}")],
        vec!["--token".into(), SECRET.into()],
        vec![SECRET.into()],
    ] {
        assert_sanitized_rejection(&args, SECRET);
    }
    for flag in [
        "--jira-url",
        "--jira-board-id",
        "--jira-projects",
        "--jira-groups",
        "--jira-team-field",
        "--jira-team",
        "--jira-sprint-name-pattern",
    ] {
        let value = format!("[{SECRET}\n");
        assert_sanitized_rejection(&[format!("{flag}={value}")], SECRET);
        assert_sanitized_rejection(&[flag.into(), value], SECRET);
    }
    for flag in ["--jira-email", "--jira-token", "--jira-token-file"] {
        assert_sanitized_rejection(&[format!("{flag}={SECRET}")], SECRET);
    }
}

fn assert_sanitized_rejection(args: &[String], secret: &str) {
    let output = binary().args(args).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(secret));
    assert!(stderr.contains("invalid command-line arguments"));
    assert!(stderr.contains("--help"));
    assert!(!output.stdout.starts_with(b"\x1b"));
}
