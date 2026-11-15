use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct FakeRunner {
    paths: Paths,
    calls: Mutex<Vec<Command>>,
    running: AtomicBool,
    present: AtomicBool,
    invocation: AtomicUsize,
    graphical: bool,
    fail: Option<Command>,
    cancel_at: Option<(Command, CancellationToken)>,
    own_port: Option<u16>,
    hold_old_invocation: bool,
    skip_generation: bool,
    container_inventory: &'static str,
}
impl FakeRunner {
    fn new(paths: Paths) -> Self {
        Self {
            paths,
            calls: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            present: AtomicBool::new(false),
            invocation: AtomicUsize::new(1),
            graphical: true,
            fail: None,
            cancel_at: None,
            own_port: None,
            hold_old_invocation: false,
            skip_generation: false,
            container_inventory: "[]",
        }
    }
    fn calls(&self) -> Vec<Command> {
        self.calls.lock().unwrap().clone()
    }
}
impl CommandRunner for FakeRunner {
    async fn run(
        &self,
        command: Command,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, InstallError> {
        check_cancel(cancel)?;
        assert!(timeout <= Duration::from_secs(900));
        self.calls.lock().unwrap().push(command);
        if self.fail == Some(command) {
            return Ok(CommandOutput {
                success: false,
                stdout: "SECRET_TOKEN not for display".into(),
            });
        }
        if let Some((at, token)) = &self.cancel_at
            && *at == command
        {
            token.cancel();
            return Err(CANCELLED);
        }
        let stdout = match command {
            Command::PodmanInfo => r#"{"host":{"security":{"rootless":true},"cgroupVersion":"v2"}}"#.into(),
            Command::QuadletVersion => "quadlet 5.4.0".into(),
            Command::UserManager => "257".into(),
            Command::GraphicalStatus => if self.graphical { "active" } else { "inactive" }.into(),
            Command::ServiceStatus => {
                if self.present.load(Ordering::SeqCst) {
                    format!("LoadState=loaded\nActiveState={}\nSourcePath={}\nFragmentPath=/run/user/{}/systemd/generator/pullboard.service\nInvocationID={:032x}\nDropInPaths=\n", if self.running.load(Ordering::SeqCst) { "active" } else { "inactive" }, self.paths.quadlet().display(), rustix::process::geteuid().as_raw(), self.invocation.load(Ordering::SeqCst))
                } else { "LoadState=not-found\nActiveState=inactive\nSourcePath=\nFragmentPath=\nInvocationID=\n".into() }
            }
            Command::ContainerStatus | Command::LegacyContainerStatus => serde_json::json!({"State":{"Running":true},"Config":{"Labels":{"PODMAN_SYSTEMD_UNIT":"pullboard.service"}},"NetworkSettings":{"Ports":{"8080/tcp":[{"HostIp":"127.0.0.1","HostPort":self.own_port.map(|p| p.to_string()).unwrap_or_default()}]}}}).to_string(),
            Command::ContainerInventory => self.container_inventory.into(),
            Command::Reload => { if !self.skip_generation { self.present.store(true, Ordering::SeqCst); } String::new() }
            Command::Start | Command::Restart => {
                self.running.store(true, Ordering::SeqCst);
                if !self.hold_old_invocation { self.invocation.fetch_add(1, Ordering::SeqCst); }
                String::new()
            }
            Command::Pull(_) => String::new(),
        };
        Ok(CommandOutput {
            success: command != Command::GraphicalStatus || self.graphical,
            stdout,
        })
    }
}

struct FakeProbe {
    health: bool,
    health_failures: usize,
    health_calls: AtomicUsize,
}
impl FakeProbe {
    fn ready() -> Self {
        Self {
            health: true,
            health_failures: 0,
            health_calls: AtomicUsize::new(0),
        }
    }
}
impl ReadinessProbe for FakeProbe {
    async fn health(&self, _: u16, cancel: &CancellationToken) -> Result<bool, InstallError> {
        check_cancel(cancel)?;
        let call = self.health_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.health && call >= self.health_failures)
    }
    async fn sources(
        &self,
        _: u16,
        _: &CancellationToken,
    ) -> Result<Option<Vec<SourceSummary>>, InstallError> {
        panic!("installation must not probe source refresh status")
    }
}
fn home() -> (tempfile::TempDir, Paths) {
    let temp = test_temp("installer-test-");
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let paths = Paths::for_home(temp.path().to_path_buf()).unwrap();
    (temp, paths)
}
fn installer(paths: Paths) -> Installer<FakeRunner, FakeProbe> {
    let mut installer = Installer::with_runtime(FakeRunner::new(paths), FakeProbe::ready());
    installer.wait = Duration::from_millis(10);
    installer.poll = Duration::from_millis(1);
    installer
}
fn test_temp(prefix: &str) -> tempfile::TempDir {
    let parent = if Path::new("/tmp/opencode").is_dir() {
        PathBuf::from("/tmp/opencode")
    } else {
        std::env::temp_dir()
    };
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .unwrap()
}
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn write_private(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn assert_no_stages(paths: &Paths) {
    for dir in [
        paths.config().parent().unwrap().to_path_buf(),
        paths.quadlet().parent().unwrap().to_path_buf(),
    ] {
        if dir.exists() {
            assert!(fs::read_dir(dir).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".pullboard-setup-")
            }));
        }
    }
}

#[test]
fn rendered_policy_matches_supplied_template_except_port_and_channel() {
    assert_eq!(render_quadlet(3333, Channel::Stable).unwrap(), TEMPLATE);
    assert!(TEMPLATE.contains("\nContainerName=pullboard\n"));
    let rendered = render_quadlet(4567, Channel::Nightly).unwrap();
    assert_eq!(
        rendered
            .replace(
                "ghcr.io/pbrezina/pullboard:nightly",
                "ghcr.io/pbrezina/pullboard:latest"
            )
            .replace("127.0.0.1:4567:8080", "127.0.0.1:3333:8080"),
        TEMPLATE
    );
    assert!(render_quadlet(80, Channel::Stable).is_err());
    assert_eq!(template_settings(&rendered), Some((4567, Channel::Nightly)));
    assert_eq!(
        template_settings(&rendered.replace("ReadOnly=true", "ReadOnly=false")),
        None
    );
}

#[test]
fn previous_exact_templates_remain_recognized_but_customizations_do_not() {
    for channel in [Channel::Stable, Channel::Nightly] {
        let legacy = render_quadlet(4567, channel)
            .unwrap()
            .replace("ContainerName=pullboard\n", "");
        assert_eq!(template_settings(&legacy), Some((4567, channel)));
        for custom in [
            legacy.replace("ReadOnly=true", "ReadOnly=false"),
            legacy.replace("[Container]\n", "[Container]\nContainerName=custom\n"),
        ] {
            assert_eq!(template_settings(&custom), None);
        }
    }
}

#[tokio::test]
async fn running_legacy_container_can_upgrade_on_its_occupied_port() {
    for (channel, save_only_first) in [
        (Channel::Stable, false),
        (Channel::Nightly, false),
        (Channel::Stable, true),
        (Channel::Nightly, true),
    ] {
        let (_temp, paths) = home();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let rendered = render_quadlet(port, channel).unwrap();
        let legacy = rendered.replace("ContainerName=pullboard\n", "");
        write_private(&paths.quadlet(), &legacy);
        let mut installer = installer(paths.clone());
        installer.runner.present.store(true, Ordering::SeqCst);
        installer.runner.running.store(true, Ordering::SeqCst);
        installer.runner.own_port = Some(port);
        installer.runner.fail = Some(Command::ContainerStatus);
        let cancel = CancellationToken::new();
        let mut state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        assert_eq!(state.existing_port, Some(port));
        assert_eq!(state.existing_channel, Some(channel));
        if save_only_first {
            let report = installer
                .install(
                    &state,
                    "secret: config",
                    port,
                    channel,
                    false,
                    &cancel,
                    |_| {},
                )
                .await;
            assert!(report.files_saved && report.failure.is_none(), "{report:?}");
            assert!(!report.service_job_requested);
            state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        }
        let report = installer
            .install(
                &state,
                "secret: config",
                port,
                channel,
                true,
                &cancel,
                |_| {},
            )
            .await;
        assert!(report.files_saved && report.http_ready, "{report:?}");
        assert!(report.failure.is_none());
        assert_eq!(fs::read_to_string(paths.quadlet()).unwrap(), rendered);
        assert_eq!(
            fs::read_to_string(paths.quadlet_backup()).unwrap(),
            if save_only_first { rendered } else { legacy }
        );
        assert!(
            installer
                .runner
                .calls()
                .contains(&Command::LegacyContainerStatus)
        );
        assert!(installer.runner.calls().contains(&Command::Restart));
        assert!(!installer.runner.calls().contains(&Command::Start));
    }
}

#[tokio::test]
async fn inspection_creates_nothing_and_warns_outside_graphical_session() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.runner.graphical = false;
    let state = installer
        .inspect(paths.clone(), &CancellationToken::new())
        .await
        .unwrap();
    assert!(!state.graphical_session);
    assert!(!state.warnings.is_empty());
    assert!(!paths.home().join(".config").exists());
    assert!(state.existing_config.is_none());
    assert!(!installer.runner.calls().iter().any(|c| matches!(
        c,
        Command::Reload | Command::Start | Command::Restart | Command::Pull(_)
    )));
}

#[tokio::test]
async fn missing_prerequisite_is_actionable_sanitized_and_never_provisioned() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.runner.fail = Some(Command::PodmanInfo);
    let error = installer
        .inspect(paths.clone(), &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("Podman"));
    assert!(!error.to_string().contains("SECRET_TOKEN"));
    assert!(!paths.home().join(".config").exists());
    assert_eq!(installer.runner.calls(), [Command::PodmanInfo]);
}

#[tokio::test]
async fn save_only_preserves_arbitrary_serialized_yaml_and_private_permissions() {
    let (_temp, paths) = home();
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let yaml = "jira:\n  token: literal-secret\nadvanced: [preserve, all, fields]\n";
    let mut events = Vec::new();
    let report = installer
        .install(
            &state,
            yaml,
            free_port(),
            Channel::Nightly,
            false,
            &cancel,
            |e| events.push(e),
        )
        .await;
    assert!(report.files_saved, "{:?}", report.failure);
    assert!(report.manager_reloaded && !report.service_job_requested);
    assert!(report.failure.is_none());
    assert_eq!(installer.probe.health_calls.load(Ordering::SeqCst), 0);
    assert!(
        !report.service_running
            && !report.http_ready
            && !report.sources_checked
            && !report.source_refresh_complete
    );
    assert_eq!(fs::read_to_string(paths.config()).unwrap(), yaml);
    for path in [paths.config(), paths.quadlet()] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    for dir in [
        ".config",
        ".config/pullboard",
        ".config/containers",
        ".config/containers/systemd",
    ] {
        assert_eq!(
            fs::metadata(paths.home().join(dir))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    assert_eq!(
        events,
        [
            Event::SavingFiles,
            Event::FilesSaved,
            Event::ReloadingManager
        ]
    );
    assert!(!format!("{report:?}{events:?}").contains("literal-secret"));
    assert!(
        !installer
            .runner
            .calls()
            .iter()
            .any(|c| matches!(c, Command::Pull(_) | Command::Start | Command::Restart))
    );
    assert_eq!(
        installer
            .runner
            .calls()
            .iter()
            .filter(|c| **c == Command::Reload)
            .count(),
        1
    );
    assert_no_stages(&paths);
}

#[tokio::test]
async fn save_only_reload_failure_retains_files_without_pulling_or_starting() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.runner.fail = Some(Command::Reload);
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let port = free_port();
    let mut events = Vec::new();
    let report = installer
        .install(
            &state,
            "retained: secret",
            port,
            Channel::Nightly,
            false,
            &cancel,
            |e| events.push(e),
        )
        .await;
    assert!(report.files_saved && report.config_saved && report.quadlet_saved);
    assert!(
        report
            .failure
            .is_some_and(|failure| failure.contains("daemon-reload"))
    );
    assert!(!report.cancelled && !report.manager_reloaded && !report.service_job_requested);
    assert!(
        !report.service_running
            && !report.http_ready
            && !report.sources_checked
            && !report.source_refresh_complete
    );
    assert_eq!(
        fs::read_to_string(paths.config()).unwrap(),
        "retained: secret"
    );
    assert_eq!(
        fs::read_to_string(paths.quadlet()).unwrap(),
        render_quadlet(port, Channel::Nightly).unwrap()
    );
    assert_eq!(
        events,
        [
            Event::SavingFiles,
            Event::FilesSaved,
            Event::ReloadingManager
        ]
    );
    let calls = installer.runner.calls();
    assert_eq!(calls.iter().filter(|c| **c == Command::Reload).count(), 1);
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Command::Pull(_) | Command::Start | Command::Restart))
    );
    assert_eq!(installer.probe.health_calls.load(Ordering::SeqCst), 0);
    assert!(!format!("{report:?}").contains("SECRET_TOKEN"));
    assert_no_stages(&paths);
}

#[tokio::test]
async fn reload_success_without_a_generated_service_is_not_install_success() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.runner.skip_generation = true;
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let report = installer
        .install(
            &state,
            "retained: secret",
            free_port(),
            Channel::Stable,
            false,
            &cancel,
            |_| {},
        )
        .await;
    assert!(report.files_saved && report.manager_reloaded);
    assert!(!report.service_job_requested && !report.service_running && !report.http_ready);
    assert!(
        report
            .failure
            .is_some_and(|failure| failure.contains("did not generate"))
    );
    assert!(
        !installer
            .runner
            .calls()
            .iter()
            .any(|c| matches!(c, Command::Pull(_) | Command::Start | Command::Restart))
    );
    assert_eq!(
        fs::read_to_string(paths.config()).unwrap(),
        "retained: secret"
    );
}

#[tokio::test]
async fn replaces_existing_files_with_exactly_one_private_previous_version() {
    let (_temp, paths) = home();
    write_private(&paths.config(), "old: secret-one\n");
    write_private(&paths.quadlet(), TEMPLATE);
    write_private(&paths.config_backup(), "previous: secret-zero\n");
    write_private(&paths.quadlet_backup(), "old quadlet backup\n");
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    assert_eq!(state.existing_config.as_deref(), Some("old: secret-one\n"));
    let report = installer
        .install(
            &state,
            "new: secret-two\n",
            free_port(),
            Channel::Stable,
            false,
            &cancel,
            |_| {},
        )
        .await;
    assert!(report.files_saved, "{:?}", report.failure);
    assert_eq!(
        fs::read_to_string(paths.config_backup()).unwrap(),
        "old: secret-one\n"
    );
    assert_eq!(
        fs::read_to_string(paths.quadlet_backup()).unwrap(),
        TEMPLATE
    );
    for path in [paths.config_backup(), paths.quadlet_backup()] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_no_stages(&paths);
}

#[tokio::test]
async fn pre_cancel_and_cancel_at_first_event_make_no_files() {
    let (_temp, paths) = home();
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    cancel.cancel();
    let report = installer
        .install(
            &state,
            "secret: value",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |_| panic!("no event before cancellation"),
        )
        .await;
    assert!(report.cancelled && !report.files_saved);
    assert!(!paths.home().join(".config").exists());
    let cancel = CancellationToken::new();
    let report = installer
        .install(
            &state,
            "secret: value",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |_| cancel.cancel(),
        )
        .await;
    assert!(report.cancelled && !report.files_saved);
    assert!(!paths.home().join(".config").exists());
}

#[tokio::test]
async fn cancellation_after_save_retains_configuration_without_starting() {
    let (_temp, paths) = home();
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let report = installer
        .install(
            &state,
            "retained: secret",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |event| {
                if event == Event::FilesSaved {
                    cancel.cancel();
                }
            },
        )
        .await;
    assert!(report.cancelled && report.files_saved);
    assert!(!report.service_running && !report.http_ready);
    assert_eq!(
        fs::read_to_string(paths.config()).unwrap(),
        "retained: secret"
    );
    assert!(!installer.runner.calls().contains(&Command::Reload));
}

#[tokio::test]
async fn changed_config_or_backup_aborts_without_overwriting_external_edit() {
    for backup in [false, true] {
        let (_temp, paths) = home();
        write_private(&paths.config(), "original: secret");
        let installer = installer(paths.clone());
        let cancel = CancellationToken::new();
        let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        let edited = if backup {
            paths.config_backup()
        } else {
            paths.config()
        };
        write_private(&edited, "external: secret");
        let report = installer
            .install(
                &state,
                "replacement: secret",
                free_port(),
                Channel::Stable,
                false,
                &cancel,
                |_| {},
            )
            .await;
        assert!(!report.files_saved && report.failure.is_some());
        assert_eq!(fs::read_to_string(edited).unwrap(), "external: secret");
        assert!(!paths.quadlet().exists());
    }
}

#[tokio::test]
async fn changed_directory_inode_and_same_content_replacement_are_conflicts() {
    for directory in [false, true] {
        let (_temp, paths) = home();
        write_private(&paths.config(), "unchanged: contents");
        let installer = installer(paths.clone());
        let cancel = CancellationToken::new();
        let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        if directory {
            fs::rename(
                paths.home().join(".config/pullboard"),
                paths.home().join(".config/moved-pullboard"),
            )
            .unwrap();
            write_private(&paths.config(), "unchanged: contents");
        } else {
            let other = paths.home().join("replacement");
            write_private(&other, "unchanged: contents");
            fs::rename(other, paths.config()).unwrap();
        }
        let report = installer
            .install(
                &state,
                "new: value",
                free_port(),
                Channel::Stable,
                false,
                &cancel,
                |_| {},
            )
            .await;
        assert!(!report.files_saved && report.failure.is_some());
        assert_eq!(
            fs::read_to_string(paths.config()).unwrap(),
            "unchanged: contents"
        );
    }
}

#[tokio::test]
async fn rejects_symlink_components_destinations_and_backup_links() {
    for relative in [
        ".config",
        ".config/pullboard/config.yaml",
        ".config/pullboard/config.yaml.bak",
        ".config/containers/systemd/pullboard.container",
    ] {
        let (_temp, paths) = home();
        let outside = test_temp("installer-outside-");
        let link = paths.home().join(relative);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        if relative == ".config" {
            symlink(outside.path(), &link).unwrap();
        } else {
            let target = outside.path().join("untouched");
            fs::write(&target, "external secret").unwrap();
            symlink(target, &link).unwrap();
        }
        let installer = installer(paths.clone());
        assert!(
            installer
                .inspect(paths.clone(), &CancellationToken::new())
                .await
                .is_err()
        );
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    }
}

#[tokio::test]
async fn rejects_hardlinked_files_world_writable_directories_and_custom_quadlets() {
    for kind in [0, 1, 2] {
        let (_temp, paths) = home();
        write_private(&paths.config(), "secret: original");
        match kind {
            0 => fs::hard_link(paths.config(), paths.home().join("linked")).unwrap(),
            1 => fs::set_permissions(
                paths.config().parent().unwrap(),
                fs::Permissions::from_mode(0o777),
            )
            .unwrap(),
            _ => write_private(
                &paths.quadlet(),
                &TEMPLATE.replace("ReadOnly=true", "ReadOnly=false"),
            ),
        }
        let installer = installer(paths.clone());
        assert!(
            installer
                .inspect(paths, &CancellationToken::new())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn refuses_custom_overrides_instead_of_starting_with_weakened_policy() {
    for relative in [
        ".config/containers/systemd/pullboard.container.d",
        ".config/containers/systemd/container.d",
        ".config/systemd/user/pullboard.service",
        ".config/systemd/user/pullboard.service.d",
        ".config/systemd/user/service.d",
    ] {
        let (_temp, paths) = home();
        let custom = paths.home().join(relative);
        write_private(&custom, "custom user work, never overwrite");
        let installer = installer(paths.clone());
        assert!(
            installer
                .inspect(paths, &CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(custom).unwrap(),
            "custom user work, never overwrite"
        );
    }
}

#[tokio::test]
async fn symlink_introduced_after_inspection_is_not_followed_or_replaced() {
    let (_temp, paths) = home();
    write_private(&paths.config(), "original: secret");
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let outside = paths.home().join("outside");
    write_private(&outside, "unrelated: secret");
    fs::remove_file(paths.config()).unwrap();
    symlink(&outside, paths.config()).unwrap();
    let report = installer
        .install(
            &state,
            "replacement: secret",
            free_port(),
            Channel::Stable,
            false,
            &cancel,
            |_| {},
        )
        .await;
    assert!(!report.config_saved && report.failure.is_some());
    assert_eq!(fs::read_to_string(outside).unwrap(), "unrelated: secret");
    assert!(fs::symlink_metadata(paths.config()).unwrap().is_symlink());
}

#[tokio::test]
async fn unrelated_occupied_port_is_rejected_but_verified_own_container_is_allowed() {
    let (_temp, paths) = home();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    write_private(
        &paths.quadlet(),
        &render_quadlet(port, Channel::Stable).unwrap(),
    );
    let mut installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    assert!(installer.check_port(&state, port, &cancel).await.is_err());
    installer.runner.present.store(true, Ordering::SeqCst);
    installer.runner.running.store(true, Ordering::SeqCst);
    assert!(installer.check_port(&state, port, &cancel).await.is_err());
    installer.runner.own_port = Some(port);
    assert!(installer.check_port(&state, port, &cancel).await.is_ok());
}

#[tokio::test]
async fn successful_start_waits_for_health_then_returns_without_source_checks() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.probe.health_failures = 2;
    installer.wait = Duration::from_secs(1);
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths, &cancel).await.unwrap();
    let mut events = Vec::new();
    let report = installer
        .install(
            &state,
            "secret: value",
            free_port(),
            Channel::Nightly,
            true,
            &cancel,
            |e| events.push(e),
        )
        .await;
    assert!(
        report.files_saved && report.service_running && report.http_ready,
        "{report:?}"
    );
    assert!(report.failure.is_none() && !report.cancelled);
    assert!(report.warnings.is_empty());
    assert!(!report.sources_checked && !report.source_refresh_complete);
    assert!(report.sources.is_empty());
    assert_eq!(installer.probe.health_calls.load(Ordering::SeqCst), 3);
    let calls = installer.runner.calls();
    let pull = calls
        .iter()
        .position(|c| *c == Command::Pull(Channel::Nightly))
        .unwrap();
    let reload = calls.iter().position(|c| *c == Command::Reload).unwrap();
    let start = calls.iter().position(|c| *c == Command::Start).unwrap();
    assert!(pull < reload && reload < start);
    assert!(!calls.contains(&Command::Restart));
    assert_eq!(
        events,
        [
            Event::SavingFiles,
            Event::FilesSaved,
            Event::PullingImage,
            Event::ImagePulled,
            Event::ReloadingManager,
            Event::StartingService,
            Event::WaitingForService,
            Event::ServiceRunning,
            Event::WaitingForHttp,
            Event::HttpReady,
        ]
    );
}

#[tokio::test]
async fn running_service_restarts_and_cannot_reuse_old_invocation_readiness() {
    for hold in [false, true] {
        let (_temp, paths) = home();
        write_private(&paths.config(), "old: config");
        write_private(&paths.quadlet(), TEMPLATE);
        let mut installer = installer(paths.clone());
        installer.runner.present.store(true, Ordering::SeqCst);
        installer.runner.running.store(true, Ordering::SeqCst);
        installer.runner.hold_old_invocation = hold;
        let cancel = CancellationToken::new();
        let state = installer.inspect(paths, &cancel).await.unwrap();
        let report = installer
            .install(
                &state,
                "new: config",
                free_port(),
                Channel::Stable,
                true,
                &cancel,
                |_| {},
            )
            .await;
        assert_eq!(report.http_ready, !hold);
        assert_eq!(report.service_running, !hold);
        assert!(installer.runner.calls().contains(&Command::Restart));
        assert!(!installer.runner.calls().contains(&Command::Start));
        if hold {
            assert_eq!(installer.probe.health_calls.load(Ordering::SeqCst), 0);
            assert!(
                report
                    .failure
                    .is_some_and(|failure| failure.contains("service did not become active"))
            );
        } else {
            assert!(report.failure.is_none());
            assert_eq!(installer.probe.health_calls.load(Ordering::SeqCst), 1);
        }
        assert!(!report.sources_checked && !report.source_refresh_complete);
        assert!(report.sources.is_empty());
    }
}

#[tokio::test]
async fn pull_reload_and_start_failure_retain_files_without_claiming_ready() {
    for failed in [
        Command::Pull(Channel::Stable),
        Command::Reload,
        Command::Start,
    ] {
        let (_temp, paths) = home();
        let mut installer = installer(paths.clone());
        installer.runner.fail = Some(failed);
        let cancel = CancellationToken::new();
        let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        let report = installer
            .install(
                &state,
                "retained: secret",
                free_port(),
                Channel::Stable,
                true,
                &cancel,
                |_| {},
            )
            .await;
        assert!(report.files_saved && report.failure.is_some());
        assert!(!report.service_running && !report.http_ready && !report.source_refresh_complete);
        assert!(!format!("{report:?}").contains("SECRET_TOKEN"));
        assert_eq!(
            fs::read_to_string(paths.config()).unwrap(),
            "retained: secret"
        );
    }
}

#[tokio::test]
async fn command_cancellation_is_reported_as_partial_completion() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    installer.runner.cancel_at = Some((Command::Pull(Channel::Stable), cancel.clone()));
    let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
    let report = installer
        .install(
            &state,
            "retained: secret",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |_| {},
        )
        .await;
    assert!(report.cancelled && report.files_saved);
    assert!(report.failure.is_none());
    assert!(!installer.runner.calls().contains(&Command::Reload));
}

#[tokio::test]
async fn active_service_without_health_readiness_is_a_failure() {
    let (_temp, paths) = home();
    let mut installer = installer(paths.clone());
    installer.probe.health = false;
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths, &cancel).await.unwrap();
    let mut events = Vec::new();
    let report = installer
        .install(
            &state,
            "secret: value",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |event| events.push(event),
        )
        .await;
    assert!(report.files_saved && report.service_running);
    assert!(!report.http_ready && !report.cancelled);
    assert!(
        report
            .failure
            .is_some_and(|failure| failure.contains("HTTP readiness timed out"))
    );
    assert!(!report.sources_checked && !report.source_refresh_complete);
    assert!(report.sources.is_empty());
    assert!(!report.warnings.is_empty());
    assert!(installer.probe.health_calls.load(Ordering::SeqCst) > 0);
    assert_eq!(events.last(), Some(&Event::WaitingForHttp));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CheckingSources | Event::Sources(_)))
    );
}

#[tokio::test]
async fn cancellation_during_health_readiness_retains_partial_completion_warnings() {
    let (_temp, paths) = home();
    let installer = installer(paths.clone());
    let cancel = CancellationToken::new();
    let state = installer.inspect(paths, &cancel).await.unwrap();
    let report = installer
        .install(
            &state,
            "secret: value",
            free_port(),
            Channel::Stable,
            true,
            &cancel,
            |event| {
                if event == Event::WaitingForHttp {
                    cancel.cancel();
                }
            },
        )
        .await;
    assert!(report.files_saved && report.service_running && report.cancelled);
    assert!(!report.http_ready && report.failure.is_none());
    assert!(!report.sources_checked && !report.source_refresh_complete);
    assert!(report.sources.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("partially completed"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("job may continue"))
    );
}

#[test]
fn home_path_and_unrelated_unit_validation_fail_closed() {
    for path in ["relative", "/", "/var/home/../other"] {
        assert!(Paths::for_home(PathBuf::from(path)).is_err());
    }
    let (_temp, paths) = home();
    let custom = CommandOutput { success: true, stdout: "LoadState=loaded\nActiveState=inactive\nSourcePath=\nFragmentPath=/home/custom/pullboard.service\n".into() };
    assert!(runtime::service(&custom, &paths).is_err());
    assert!(
        runtime::service(
            &CommandOutput {
                success: false,
                stdout: "raw secret".into()
            },
            &paths
        )
        .is_err()
    );
}

#[test]
fn resolves_parent_home_aliases_but_rejects_a_symlinked_home_itself() {
    let (temp, real_paths) = home();
    let parent = test_temp("installer-parent-");
    let alias = parent.path().join("alias");
    symlink(temp.path().parent().unwrap(), &alias).unwrap();
    let through_alias = alias.join(temp.path().file_name().unwrap());
    let resolved = Paths::for_home(through_alias).unwrap();
    assert_eq!(resolved.home(), real_paths.home());
    let leaf_alias = parent.path().join("home-link");
    symlink(temp.path(), &leaf_alias).unwrap();
    assert!(Paths::for_home(leaf_alias).is_err());
}

#[tokio::test]
async fn unrelated_reserved_container_is_never_replaced_even_with_free_port() {
    for appeared_later in [false, true] {
        let (_temp, paths) = home();
        let mut installer = installer(paths.clone());
        let cancel = CancellationToken::new();
        let state = installer.inspect(paths.clone(), &cancel).await.unwrap();
        installer.runner.container_inventory =
            r#"[{"Names":["pullboard"],"Labels":{"application":"other-user-work"}}]"#;
        if appeared_later {
            let report = installer
                .install(
                    &state,
                    "secret: config",
                    free_port(),
                    Channel::Stable,
                    true,
                    &cancel,
                    |_| {},
                )
                .await;
            assert!(report.failure.is_some() && !report.files_saved);
        } else {
            assert!(installer.inspect(paths.clone(), &cancel).await.is_err());
        }
        assert!(!paths.home().join(".config").exists());
        assert!(!installer.runner.calls().contains(&Command::Start));
    }
}

#[test]
fn only_known_template_owned_container_name_can_be_reused() {
    let owned = r#"[{"Names":["pullboard"],"Labels":{"PODMAN_SYSTEMD_UNIT":"pullboard.service"}}]"#;
    assert!(runtime::container_name_safe("[]", false));
    assert!(runtime::container_name_safe(owned, true));
    assert!(!runtime::container_name_safe(owned, false));
    assert!(!runtime::container_name_safe(
        &owned.replace("\"pullboard\"", "\"systemd-pullboard\""),
        true
    ));
    assert!(!runtime::container_name_safe("not-json SECRET_TOKEN", true));
    assert!(!runtime::container_name_safe("{}", true));
}

#[test]
fn transitional_existing_service_is_not_treated_as_stopped_or_newly_ready() {
    let (_temp, paths) = home();
    for active in ["activating", "deactivating", "reloading", "maintenance"] {
        let output = CommandOutput {
            success: true,
            stdout: format!(
                "LoadState=loaded\nActiveState={active}\nSourcePath={}\nFragmentPath=/run/user/{}/systemd/generator/pullboard.service\nInvocationID={:032x}\nDropInPaths=\n",
                paths.quadlet().display(),
                rustix::process::geteuid().as_raw(),
                1
            ),
        };
        let service = runtime::service(&output, &paths).unwrap();
        assert!(service.changing);
        assert!(require_settled(&service).is_err());
    }
}
