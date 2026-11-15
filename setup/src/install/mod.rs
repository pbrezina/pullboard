//! Rootless Linux Quadlet installation. No shell, privileged provisioning, or forge writes.
//!
//! The caller owns schema validation and final user confirmation. Serialized YAML is
//! copied verbatim and deliberately never included in Debug, errors, or progress.
#![forbid(unsafe_code)]

mod files;
mod runtime;
#[cfg(test)]
mod tests;

pub use runtime::{HttpProbe, SystemRunner};
use std::{
    fmt,
    future::Future,
    net::{Ipv4Addr, TcpListener},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const TEMPLATE: &str = include_str!("../../../deploy/systemd/pullboard.container");
const MAX_FILE: usize = 1024 * 1024;

/// Fixed host locations; callers cannot select arbitrary installation targets.
#[derive(Clone, Debug)]
pub struct Paths {
    home: PathBuf,
    alias: PathBuf,
}

impl Paths {
    /// Accept an existing absolute home directory, resolving parent aliases such as
    /// Fedora's /home -> /var/home. The home itself and paths below it cannot be symlinks.
    pub fn for_home(home: PathBuf) -> Result<Self, InstallError> {
        if !home.is_absolute()
            || home == Path::new("/")
            || home
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(InstallError(
                "home must be an absolute normalized directory",
            ));
        }
        let metadata = std::fs::symlink_metadata(&home)
            .map_err(|_| InstallError("home directory is unavailable"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(InstallError(
                "home must be a real directory, not a symbolic link",
            ));
        }
        let canonical = std::fs::canonicalize(&home)
            .map_err(|_| InstallError("home directory could not be resolved safely"))?;
        Ok(Self {
            home: canonical,
            alias: home,
        })
    }
    pub fn home(&self) -> &Path {
        &self.home
    }
    pub fn config(&self) -> PathBuf {
        self.home.join(".config/pullboard/config.yaml")
    }
    pub fn quadlet(&self) -> PathBuf {
        self.home
            .join(".config/containers/systemd/pullboard.container")
    }
    pub fn config_backup(&self) -> PathBuf {
        self.home.join(".config/pullboard/config.yaml.bak")
    }
    pub fn quadlet_backup(&self) -> PathBuf {
        self.home
            .join(".config/containers/systemd/pullboard.container.bak")
    }
}

/// Only fixed publisher-controlled image names are accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    Stable,
    Nightly,
}
impl Channel {
    pub fn image(self) -> &'static str {
        match self {
            Self::Stable => "ghcr.io/pbrezina/pullboard:latest",
            Self::Nightly => "ghcr.io/pbrezina/pullboard:nightly",
        }
    }
}

/// Render the supplied hardening policy, changing only host port and image channel.
pub fn render_quadlet(port: u16, channel: Channel) -> Result<String, InstallError> {
    validate_port(port)?;
    Ok(TEMPLATE
        .replace("ghcr.io/pbrezina/pullboard:latest", channel.image())
        .replace("127.0.0.1:3333:8080", &format!("127.0.0.1:{port}:8080")))
}

fn validate_port(port: u16) -> Result<(), InstallError> {
    if port < 1024 {
        Err(InstallError(
            "select an unprivileged host port between 1024 and 65535",
        ))
    } else {
        Ok(())
    }
}

fn template_settings(text: &str) -> Option<(u16, Channel)> {
    let port = text.lines().find_map(|l| {
        l.strip_prefix("PublishPort=127.0.0.1:")?
            .strip_suffix(":8080")?
            .parse::<u16>()
            .ok()
    })?;
    [Channel::Stable, Channel::Nightly]
        .into_iter()
        .find(|c| {
            render_quadlet(port, *c).is_ok_and(|rendered| {
                rendered == text
                    // Accept the previous exact template so setup can upgrade it.
                    || rendered.replace("ContainerName=pullboard\n", "") == text
            })
        })
        .map(|channel| (port, channel))
}

/// Safe user-facing error, without secret-bearing source chains or subprocess output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstallError(pub(crate) &'static str);
impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for InstallError {}
const CANCELLED: InstallError = InstallError("operation cancelled");

/// Secret-bearing inspection result: intentionally not Debug or serializable.
pub struct State {
    pub paths: Paths,
    pub existing_config: Option<String>,
    pub quadlet_exists: bool,
    pub existing_port: Option<u16>,
    pub existing_channel: Option<Channel>,
    pub service_running: bool,
    pub graphical_session: bool,
    pub warnings: Vec<&'static str>,
    snapshot: files::Snapshot,
}

/// Progress records are actual operations, never estimated percentages or raw command output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    SavingFiles,
    FilesSaved,
    PullingImage,
    ImagePulled,
    ReloadingManager,
    StartingService,
    RestartingService,
    WaitingForService,
    ServiceRunning,
    WaitingForHttp,
    HttpReady,
    // Retained for caller compatibility; installation no longer checks sources.
    CheckingSources,
    Sources(Vec<SourceSummary>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceStatus {
    Pending,
    Ok,
    Error,
    Backoff,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSummary {
    pub source_id: String,
    pub status: SourceStatus,
    pub refreshing: bool,
}

/// Always returned, including partial completion. Saved files do not imply service readiness.
#[derive(Debug)]
pub struct InstallReport {
    pub paths: Paths,
    pub url: String,
    pub config_saved: bool,
    pub quadlet_saved: bool,
    pub files_saved: bool,
    pub manager_reloaded: bool,
    pub service_job_requested: bool,
    pub service_running: bool,
    pub http_ready: bool,
    // Retained for caller compatibility; source refresh is not part of readiness.
    pub sources: Vec<SourceSummary>,
    pub sources_checked: bool,
    pub source_refresh_complete: bool,
    pub cancelled: bool,
    pub failure: Option<&'static str>,
    pub graphical_session: bool,
    pub warnings: Vec<&'static str>,
}

/// A fixed read-only or explicitly confirmed runtime command. No arbitrary executable/arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    PodmanInfo,
    QuadletVersion,
    UserManager,
    ServiceStatus,
    GraphicalStatus,
    ContainerStatus,
    LegacyContainerStatus,
    ContainerInventory,
    Pull(Channel),
    Reload,
    Start,
    Restart,
}

/// Captured stdout is bounded and never forwarded to the UI. Do not Debug this type.
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
}

/// Injection boundary for unit tests. Implementations must honor cancellation and timeout.
pub trait CommandRunner: Send + Sync {
    fn run(
        &self,
        command: Command,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<CommandOutput, InstallError>> + Send;
}

/// Bounded local HTTP observations; no upstream requests and no refresh-triggering POSTs.
pub trait ReadinessProbe: Send + Sync {
    fn health(
        &self,
        port: u16,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<bool, InstallError>> + Send;
    /// Retained for caller compatibility; the installer never calls this method.
    fn sources(
        &self,
        port: u16,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<Vec<SourceSummary>>, InstallError>> + Send;
}

/// Installer with injectable command and HTTP boundaries.
pub struct Installer<R = SystemRunner, P = HttpProbe> {
    runner: R,
    probe: P,
    wait: Duration,
    poll: Duration,
}
impl Installer {
    pub fn new() -> Result<Self, InstallError> {
        Ok(Self::with_runtime(SystemRunner, HttpProbe::new()?))
    }
}

impl<R: CommandRunner, P: ReadinessProbe> Installer<R, P> {
    pub fn with_runtime(runner: R, probe: P) -> Self {
        Self {
            runner,
            probe,
            wait: Duration::from_secs(120),
            poll: Duration::from_secs(1),
        }
    }

    /// Read files/snapshots and check prerequisites without provisioning or writing anything.
    pub async fn inspect(
        &self,
        paths: Paths,
        cancel: &CancellationToken,
    ) -> Result<State, InstallError> {
        check_cancel(cancel)?;
        let read_paths = paths.clone();
        let snapshot = tokio::task::spawn_blocking(move || files::Snapshot::inspect(&read_paths))
            .await
            .map_err(|_| InstallError("file inspection failed"))??;
        let existing_config = snapshot.config_text()?;
        let existing_settings = snapshot.quadlet_text()?.map(|text| template_settings(&text).ok_or(InstallError("existing Quadlet is custom or unsupported; preserve it and resolve manually before setup"))).transpose()?;
        let existing_port = existing_settings.map(|(port, _)| port);
        let existing_channel = existing_settings.map(|(_, channel)| channel);
        self.required(Command::PodmanInfo, cancel, "Podman is unavailable or rootless Podman cannot run; install/configure Podman manually").await.and_then(|o| {
            let info: serde_json::Value = serde_json::from_str(&o.stdout).map_err(|_| InstallError("Podman did not return valid rootless runtime information"))?;
            if info.pointer("/host/security/rootless").and_then(|v| v.as_bool()) != Some(true) { return Err(InstallError("setup requires rootless Podman; do not run setup as root")); }
            if info.pointer("/host/cgroupVersion").and_then(|v| v.as_str()) != Some("v2") { return Err(InstallError("Quadlet requires cgroup v2; configure a supported host manually before setup")); }
            Ok(())
        })?;
        self.required(
            Command::QuadletVersion,
            cancel,
            "Quadlet is unavailable; install a Podman package with user Quadlet support manually",
        )
        .await?;
        self.required(
            Command::UserManager,
            cancel,
            "systemd user manager is unavailable; use a user login session with systemd --user",
        )
        .await?;
        let service = self.service(&paths, cancel).await?;
        require_settled(&service)?;
        self.check_container(existing_port.is_some(), cancel)
            .await?;
        let graphical = self
            .runner
            .run(Command::GraphicalStatus, Duration::from_secs(15), cancel)
            .await?;
        let graphical_session = graphical.success && graphical.stdout.trim() == "active";
        let mut warnings = Vec::new();
        if !graphical_session {
            warnings.push("no active graphical-session target; installation is allowed but graphical-login autostart is unverified");
        }
        if snapshot.config_is_readable_by_others() {
            warnings.push("existing configuration is readable by other users; replacement and backup will be private (0600)");
        }
        if snapshot.will_replace_backups() {
            warnings.push("existing previous-version .bak files will be replaced after final confirmation; only one prior version is retained");
        }
        check_cancel(cancel)?;
        Ok(State {
            paths,
            existing_config,
            quadlet_exists: existing_port.is_some(),
            existing_port,
            existing_channel,
            service_running: service.running,
            graphical_session,
            warnings,
            snapshot,
        })
    }

    /// An occupied port is permitted only for the verified existing generated service/container.
    pub async fn check_port(
        &self,
        state: &State,
        port: u16,
        cancel: &CancellationToken,
    ) -> Result<(), InstallError> {
        check_cancel(cancel)?;
        validate_port(port)?;
        if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Ok(());
        }
        if state.existing_port == Some(port) && self.service(&state.paths, cancel).await?.running {
            // A save-only upgrade can leave the previous container running until restart.
            for command in [Command::ContainerStatus, Command::LegacyContainerStatus] {
                let output = self
                    .runner
                    .run(command, Duration::from_secs(15), cancel)
                    .await?;
                if output.success && runtime::owns_port(&output.stdout, port) {
                    return Ok(());
                }
            }
        }
        Err(InstallError(
            "host port is unavailable; choose another port or stop its unrelated owner",
        ))
    }

    /// Save validated serialized YAML unchanged and reload Quadlet registration;
    /// optionally pull the image first, then start/restart the generated service.
    /// Cancellation after saving leaves files intact and explicitly reports partial completion.
    #[allow(clippy::too_many_arguments)]
    pub async fn install(
        &self,
        state: &State,
        config_yaml: &str,
        port: u16,
        channel: Channel,
        start: bool,
        cancel: &CancellationToken,
        mut events: impl FnMut(Event),
    ) -> InstallReport {
        let mut report = InstallReport {
            paths: state.paths.clone(),
            url: format!("http://127.0.0.1:{port}"),
            config_saved: false,
            quadlet_saved: false,
            files_saved: false,
            manager_reloaded: false,
            service_job_requested: false,
            service_running: false,
            http_ready: false,
            sources: Vec::new(),
            sources_checked: false,
            source_refresh_complete: false,
            cancelled: false,
            failure: None,
            graphical_session: state.graphical_session,
            warnings: state.warnings.clone(),
        };
        let result = self
            .perform(
                state,
                config_yaml,
                port,
                channel,
                start,
                cancel,
                &mut events,
                &mut report,
            )
            .await;
        if let Err(error) = result {
            report.cancelled = error == CANCELLED;
            if !report.cancelled {
                report.failure = Some(error.0);
            }
            if report.config_saved || report.quadlet_saved {
                report.warnings.push("installation was partially completed; saved files remain in place; inspect status before retrying");
            }
            if report.service_job_requested {
                report.warnings.push("a systemd start/restart job may continue after setup cancellation or failure; use systemctl --user status pullboard.service");
            }
        }
        report
    }

    #[allow(clippy::too_many_arguments)]
    async fn perform(
        &self,
        state: &State,
        yaml: &str,
        port: u16,
        channel: Channel,
        start: bool,
        cancel: &CancellationToken,
        events: &mut impl FnMut(Event),
        report: &mut InstallReport,
    ) -> Result<(), InstallError> {
        check_cancel(cancel)?;
        if yaml.is_empty() || yaml.len() > MAX_FILE {
            return Err(InstallError(
                "serialized configuration is empty or exceeds the 1 MiB installation limit",
            ));
        }
        let quadlet = render_quadlet(port, channel)?;
        self.check_port(state, port, cancel).await?;
        require_settled(&self.service(&state.paths, cancel).await?)?;
        self.check_container(state.quadlet_exists, cancel).await?;
        check_cancel(cancel)?;
        events(Event::SavingFiles);
        let snapshot = state.snapshot.clone();
        let paths = state.paths.clone();
        let yaml = yaml.to_owned();
        let file_cancel = cancel.clone();
        let saved = tokio::task::spawn_blocking(move || {
            files::save(&paths, &snapshot, &yaml, &quadlet, &file_cancel)
        })
        .await
        .map_err(|_| {
            InstallError("file installation worker failed; inspect files before retrying")
        })?;
        report.config_saved = saved.config;
        report.quadlet_saved = saved.quadlet;
        report.files_saved = saved.config && saved.quadlet;
        if saved.changed && saved.result.is_err() {
            report
                .warnings
                .push("installation directories or previous-version backups may have been updated");
        }
        saved.result?;
        events(Event::FilesSaved);
        check_cancel(cancel)?;
        if start {
            events(Event::PullingImage);
            self.required(Command::Pull(channel), cancel, "image pull failed; configuration is saved; check Podman connectivity and retry manually").await?;
            events(Event::ImagePulled);
        }
        events(Event::ReloadingManager);
        self.required(
            Command::Reload,
            cancel,
            "systemd user daemon-reload failed; files are saved; reload manually before starting",
        )
        .await?;
        report.manager_reloaded = true;
        check_cancel(cancel)?;
        let service = self.service(&state.paths, cancel).await?;
        if !service.loaded {
            return Err(InstallError(
                "Quadlet did not generate pullboard.service; files are saved; check Podman generator compatibility and user unit search paths",
            ));
        }
        require_settled(&service)?;
        if !start {
            report.warnings.push("files saved and user manager reloaded to register graphical-session autostart; no service start/restart requested; service state, HTTP readiness and source refresh were not verified");
            return Ok(());
        }
        self.check_container(state.quadlet_exists, cancel).await?;
        events(if service.running {
            Event::RestartingService
        } else {
            Event::StartingService
        });
        report.service_job_requested = true;
        self.required(if service.running { Command::Restart } else { Command::Start }, cancel, "service start/restart request failed; files remain saved; inspect systemctl --user status pullboard.service").await?;
        events(Event::WaitingForService);
        let deadline = tokio::time::Instant::now() + self.wait;
        loop {
            let observed = self.service(&state.paths, cancel).await?;
            // --no-block may initially still expose the old active process. Never
            // mistake that invocation's HTTP response for the newly saved config.
            if observed.running
                && (service.invocation.is_none() || observed.invocation != service.invocation)
            {
                report.service_running = true;
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(InstallError(
                    "service did not become active before the deadline; inspect systemctl --user status pullboard.service",
                ));
            }
            self.pause(cancel).await?;
        }
        events(Event::ServiceRunning);
        events(Event::WaitingForHttp);
        let deadline = tokio::time::Instant::now() + self.wait;
        loop {
            if self.probe.health(port, cancel).await? {
                report.http_ready = true;
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(InstallError(
                    "service is active but local HTTP readiness timed out; check the service status",
                ));
            }
            self.pause(cancel).await?;
        }
        events(Event::HttpReady);
        Ok(())
    }

    async fn pause(&self, cancel: &CancellationToken) -> Result<(), InstallError> {
        tokio::select! { biased; _ = cancel.cancelled() => Err(CANCELLED), _ = tokio::time::sleep(self.poll) => Ok(()) }
    }
    async fn required(
        &self,
        command: Command,
        cancel: &CancellationToken,
        failure: &'static str,
    ) -> Result<CommandOutput, InstallError> {
        let timeout = if matches!(command, Command::Pull(_)) {
            Duration::from_secs(900)
        } else {
            Duration::from_secs(30)
        };
        let output = self
            .runner
            .run(command, timeout, cancel)
            .await
            .map_err(|e| {
                if e == CANCELLED {
                    e
                } else {
                    InstallError(failure)
                }
            })?;
        if output.success {
            Ok(output)
        } else {
            Err(InstallError(failure))
        }
    }
    async fn service(
        &self,
        paths: &Paths,
        cancel: &CancellationToken,
    ) -> Result<runtime::Service, InstallError> {
        let output = self
            .runner
            .run(Command::ServiceStatus, Duration::from_secs(15), cancel)
            .await?;
        runtime::service(&output, paths)
    }

    async fn check_container(
        &self,
        existing_quadlet: bool,
        cancel: &CancellationToken,
    ) -> Result<(), InstallError> {
        let output = self.required(Command::ContainerInventory, cancel, "could not inspect the reserved container name; resolve Podman runtime errors manually before installation").await?;
        if runtime::container_name_safe(&output.stdout, existing_quadlet) {
            Ok(())
        } else {
            Err(InstallError(
                "an unrelated or unverifiable pullboard container exists; preserve it and resolve the container name conflict manually",
            ))
        }
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), InstallError> {
    if cancel.is_cancelled() {
        Err(CANCELLED)
    } else {
        Ok(())
    }
}

fn require_settled(service: &runtime::Service) -> Result<(), InstallError> {
    if service.changing {
        Err(InstallError(
            "the existing service is starting/stopping/reloading; wait for it to settle before installing",
        ))
    } else {
        Ok(())
    }
}
