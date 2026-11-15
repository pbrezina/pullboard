use super::*;

#[derive(Default)]
struct Output {
    lines: Vec<String>,
    emphasized: Vec<String>,
}

impl Prompts for Output {
    fn say(&mut self, message: &str) {
        self.lines.push(message.into());
    }
    fn emphasize(&mut self, message: &str) {
        self.emphasized.push(message.into());
        self.say(message);
    }
    async fn text(&mut self, _: &str, _: &str) -> Result<String> {
        panic!("reports never prompt")
    }
    async fn password(&mut self, _: bool) -> Result<String> {
        panic!("reports never prompt")
    }
    async fn confirm(&mut self, _: &str, _: bool) -> Result<bool> {
        panic!("reports never prompt")
    }
    async fn select(&mut self, _: &str, _: &[Choice], _: usize) -> Result<usize> {
        panic!("reports never prompt")
    }
    async fn multi(&mut self, _: &str, _: &[Choice], _: &[usize]) -> Result<Vec<usize>> {
        panic!("reports never prompt")
    }
}

fn ready_report() -> install::InstallReport {
    install::InstallReport {
        paths: Paths::for_home(std::env::current_dir().unwrap()).unwrap(),
        url: "http://127.0.0.1:3333".into(),
        config_saved: true,
        quadlet_saved: true,
        files_saved: true,
        manager_reloaded: true,
        service_job_requested: true,
        service_running: true,
        http_ready: true,
        sources: vec![],
        sources_checked: false,
        source_refresh_complete: false,
        cancelled: false,
        failure: None,
        graphical_session: true,
        warnings: vec![],
    }
}

#[test]
fn successful_start_is_concise_emphasizes_url_and_does_not_claim_refresh_success() {
    for test in [true, false] {
        let mut output = Output::default();
        print_report(&mut output, &ready_report(), test);
        assert_eq!(output.lines.len(), 3);
        assert_eq!(output.lines[0], "PullBoard started.");
        assert_eq!(output.emphasized, ["http://127.0.0.1:3333"]);
        assert!(output.lines[2].contains(if test {
            "Test (simulated)"
        } else {
            "Live (explicit submissions only)"
        }));
        assert!(output.lines[2].contains("Initial data loads in the background"));
    }
}

#[test]
fn success_retains_actual_warnings_without_verbose_recovery_boilerplate() {
    let mut report = ready_report();
    report
        .warnings
        .push("graphical-login autostart is unverified");
    let mut output = Output::default();
    print_report(&mut output, &report, true);
    assert_eq!(output.lines.len(), 4);
    assert_eq!(
        output.lines[3],
        "Warning: graphical-login autostart is unverified"
    );
    assert!(!output.lines.iter().any(|line| line.contains("Recovery:")));
}

#[test]
fn install_only_does_not_claim_service_or_dashboard_readiness() {
    let mut report = ready_report();
    report.service_job_requested = false;
    report.service_running = false;
    report.http_ready = false;
    let mut output = Output::default();
    print_report(&mut output, &report, true);
    assert_eq!(output.lines[0], "PullBoard installed.");
    assert!(output.lines[1].contains("no service start/restart requested"));
    assert!(output.emphasized.is_empty());
    assert!(!output.lines.iter().any(|line| line.contains("background")));
}

#[test]
fn failure_or_cancellation_keeps_partial_completion_and_recovery_details() {
    for cancelled in [true, false] {
        let mut report = ready_report();
        report.cancelled = cancelled;
        report.http_ready = false;
        if !cancelled {
            report.failure = Some("local HTTP readiness timed out");
        }
        let mut output = Output::default();
        print_report(&mut output, &report, true);
        let text = output.lines.join("\n");
        assert!(text.contains("Configuration saved: true"));
        assert!(text.contains("HTTP ready: false"));
        assert!(text.contains(if cancelled {
            "Cancelled."
        } else {
            "Failure: local HTTP readiness timed out"
        }));
        assert!(text.contains("systemctl --user status pullboard.service"));
        assert!(text.contains("journalctl --user -u pullboard.service"));
        assert!(!text.contains("source refresh complete"));
    }
}
