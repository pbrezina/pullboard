use super::{
    CANCELLED, Command, CommandOutput, CommandRunner, InstallError, MAX_FILE, Paths,
    ReadinessProbe, SourceStatus, SourceSummary, check_cancel,
};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command as Process};
use tokio_util::sync::CancellationToken;

/// Real command runner. All commands are fixed; stdout is bounded and stderr discarded.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    async fn run(
        &self,
        command: Command,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, InstallError> {
        check_cancel(cancel)?;
        let (program, args) = invocation(command)?;
        run_bounded(program, &args, timeout, cancel).await
    }
}

async fn run_bounded(
    program: &str,
    args: &[&str],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<CommandOutput, InstallError> {
    check_cancel(cancel)?;
    let mut child = Process::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            InstallError("required command could not be executed; check the prerequisites")
        })?;
    let mut stdout = child.stdout.take().ok_or(InstallError(
        "could not capture prerequisite command output",
    ))?;
    let operation = async {
        let reader = async {
            let mut bytes = Vec::new();
            (&mut stdout)
                .take((MAX_FILE + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| InstallError("could not read command result"))?;
            if bytes.len() > MAX_FILE {
                return Err(InstallError("command output exceeded the safe size limit"));
            }
            String::from_utf8(bytes)
                .map_err(|_| InstallError("command returned an unsupported result"))
        };
        let (status, output) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .map_err(|_| InstallError("command did not complete"))
            },
            reader
        )?;
        Ok(CommandOutput {
            success: status.success(),
            stdout: output,
        })
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CANCELLED),
        result = tokio::time::timeout(timeout, operation) => result.map_err(|_| InstallError("command timed out; inspect service state before retrying"))?,
    }
}

fn invocation(command: Command) -> Result<(&'static str, Vec<&'static str>), InstallError> {
    Ok(match command {
        Command::PodmanInfo => ("podman", vec!["--remote=false", "info", "--format=json"]),
        Command::QuadletVersion => {
            let generator = [
                "/usr/libexec/podman/quadlet",
                "/usr/lib/systemd/system-generators/podman-system-generator",
                "/usr/libexec/podman/podman-system-generator",
                "/usr/lib/systemd/user-generators/podman-user-generator",
            ]
            .into_iter()
            .find(|p| Path::new(p).is_file())
            .ok_or(InstallError(
                "Quadlet generator was not found; install a supported Podman package manually",
            ))?;
            (generator, vec!["--version"])
        }
        Command::UserManager => (
            "systemctl",
            vec!["--user", "show", "--property=Version", "--value"],
        ),
        Command::ServiceStatus => (
            "systemctl",
            vec![
                "--user",
                "show",
                "pullboard.service",
                "--property=LoadState,ActiveState,SourcePath,FragmentPath,InvocationID",
            ],
        ),
        Command::GraphicalStatus => (
            "systemctl",
            vec!["--user", "is-active", "graphical-session.target"],
        ),
        Command::ContainerStatus => (
            "podman",
            vec![
                "--remote=false",
                "container",
                "inspect",
                "pullboard",
                "--format={{json .}}",
            ],
        ),
        Command::LegacyContainerStatus => (
            "podman",
            vec![
                "--remote=false",
                "container",
                "inspect",
                "systemd-pullboard",
                "--format={{json .}}",
            ],
        ),
        Command::ContainerInventory => (
            "podman",
            vec![
                "--remote=false",
                "ps",
                "--all",
                "--filter=name=^pullboard$",
                "--format=json",
            ],
        ),
        Command::Pull(channel) => ("podman", vec!["--remote=false", "pull", channel.image()]),
        Command::Reload => ("systemctl", vec!["--user", "daemon-reload"]),
        Command::Start => (
            "systemctl",
            vec!["--user", "--no-block", "start", "pullboard.service"],
        ),
        Command::Restart => (
            "systemctl",
            vec!["--user", "--no-block", "restart", "pullboard.service"],
        ),
    })
}

pub(super) struct Service {
    pub loaded: bool,
    pub running: bool,
    pub invocation: Option<String>,
    pub changing: bool,
}
pub(super) fn service(output: &CommandOutput, paths: &Paths) -> Result<Service, InstallError> {
    let fields: std::collections::HashMap<_, _> = output
        .stdout
        .lines()
        .filter_map(|l| l.split_once('='))
        .collect();
    if fields.get("LoadState") == Some(&"not-found") {
        return Ok(Service {
            loaded: false,
            running: false,
            invocation: None,
            changing: false,
        });
    }
    if !output.success || !matches!(fields.get("LoadState"), Some(&"loaded")) {
        return Err(InstallError(
            "could not inspect the generated user service; resolve masked or invalid pullboard.service manually",
        ));
    }
    // Drop-ins remain under systemd/user control and are preserved by setup.
    // Verify the generated base unit's ownership rather than restricting overrides.
    // Never replace/start a same-named hand-written unit, including an inactive one.
    let source = fields.get("SourcePath").ok_or(InstallError(
        "existing service ownership could not be verified",
    ))?;
    let fragment = fields.get("FragmentPath").ok_or(InstallError(
        "existing service ownership could not be verified",
    ))?;
    let uid = rustix::process::geteuid().as_raw();
    let expected_fragment = format!("/run/user/{uid}/systemd/generator/pullboard.service");
    let original_quadlet = paths
        .alias
        .join(".config/containers/systemd/pullboard.container");
    if (Path::new(source) != paths.quadlet() && Path::new(source) != original_quadlet)
        || *fragment != expected_fragment
    {
        return Err(InstallError(
            "an unrelated or custom pullboard.service exists; preserve it and resolve the name conflict manually",
        ));
    }
    let active = fields
        .get("ActiveState")
        .copied()
        .ok_or(InstallError("service state is unavailable"))?;
    if !matches!(
        active,
        "active"
            | "inactive"
            | "failed"
            | "activating"
            | "deactivating"
            | "reloading"
            | "refreshing"
            | "maintenance"
    ) {
        return Err(InstallError("service returned an unsupported active state"));
    }
    let invocation = fields
        .get("InvocationID")
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| (*s).to_owned());
    if active == "active" && invocation.is_none() {
        return Err(InstallError(
            "active service invocation could not be verified",
        ));
    }
    Ok(Service {
        loaded: true,
        running: active == "active",
        invocation,
        changing: !matches!(active, "active" | "inactive" | "failed"),
    })
}

pub(super) fn owns_port(stdout: &str, port: u16) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return false;
    };
    let label = value
        .pointer("/Config/Labels/PODMAN_SYSTEMD_UNIT")
        .and_then(|v| v.as_str());
    if label != Some("pullboard.service")
        || value.pointer("/State/Running").and_then(|v| v.as_bool()) != Some(true)
    {
        return false;
    }
    value
        .pointer("/NetworkSettings/Ports/8080~1tcp")
        .and_then(|v| v.as_array())
        .is_some_and(|bindings| {
            bindings.len() == 1
                && bindings[0].get("HostIp").and_then(|v| v.as_str()) == Some("127.0.0.1")
                && bindings[0]
                    .get("HostPort")
                    .and_then(|v| v.as_str())
                    .is_some_and(|p| p.parse::<u16>().ok() == Some(port))
        })
}

pub(super) fn container_name_safe(stdout: &str, existing_quadlet: bool) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return false;
    };
    let Some(containers) = value.as_array() else {
        return false;
    };
    if containers.is_empty() {
        return true;
    }
    existing_quadlet
        && containers.len() == 1
        && containers[0]
            .get("Names")
            .and_then(|n| n.as_array())
            .is_some_and(|names| names.len() == 1 && names[0].as_str() == Some("pullboard"))
        && containers[0]
            .pointer("/Labels/PODMAN_SYSTEMD_UNIT")
            .and_then(|s| s.as_str())
            == Some("pullboard.service")
}

/// Local-only bounded readiness client. Proxy environment and redirects are disabled.
pub struct HttpProbe {
    client: reqwest::Client,
}
impl HttpProbe {
    pub fn new() -> Result<Self, InstallError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(3))
                .connect_timeout(Duration::from_secs(1))
                .pool_max_idle_per_host(1)
                .build()
                .map_err(|_| InstallError("could not initialize local readiness checks"))?,
        })
    }
    async fn get(
        &self,
        port: u16,
        endpoint: &'static str,
        cancel: &CancellationToken,
    ) -> Result<Option<serde_json::Value>, InstallError> {
        check_cancel(cancel)?;
        let operation = async {
            let Ok(mut response) = self
                .client
                .get(format!("http://127.0.0.1:{port}/api/{endpoint}"))
                .send()
                .await
            else {
                return Ok(None);
            };
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|n| n > MAX_FILE as u64)
            {
                return Ok(None);
            }
            let mut body = Vec::new();
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) if body.len().saturating_add(chunk.len()) <= MAX_FILE => {
                        body.extend_from_slice(&chunk)
                    }
                    Ok(None) => break,
                    _ => return Ok(None),
                }
            }
            Ok(serde_json::from_slice(&body).ok())
        };
        tokio::select! { biased; _ = cancel.cancelled() => Err(CANCELLED), result = operation => result }
    }
}
impl ReadinessProbe for HttpProbe {
    async fn health(&self, port: u16, cancel: &CancellationToken) -> Result<bool, InstallError> {
        Ok(self.get(port, "health", cancel).await?.is_some_and(|v| {
            v.get("status").and_then(|s| s.as_str()) == Some("ok")
                && v.get("version").is_some_and(|s| s.is_string())
                && v.get("read_only_test_mode").is_some_and(|s| s.is_boolean())
                && v.get("mode_revision")
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| !s.is_empty())
        }))
    }
    async fn sources(
        &self,
        port: u16,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<SourceSummary>>, InstallError> {
        let Some(value) = self.get(port, "sync", cancel).await? else {
            return Ok(None);
        };
        Ok(parse_sources(&value))
    }
}

fn parse_sources(value: &serde_json::Value) -> Option<Vec<SourceSummary>> {
    if !matches!(value.get("state")?.as_str()?, "idle" | "syncing") {
        return None;
    }
    let values = value.get("sources")?.as_array()?;
    let mut ids = std::collections::HashSet::new();
    values
        .iter()
        .map(|source| {
            let id = source.get("source_id")?.as_str()?;
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_:./".contains(&c))
                || !ids.insert(id)
            {
                return None;
            }
            let status = match source.get("status")?.as_str()? {
                "pending" => SourceStatus::Pending,
                "ok" => SourceStatus::Ok,
                "error" => SourceStatus::Error,
                "backoff" => SourceStatus::Backoff,
                _ => return None,
            };
            let refreshing = value.get("state")?.as_str()? == "syncing";
            Some(SourceSummary {
                source_id: id.to_owned(),
                status,
                refreshing,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::Channel;
    #[test]
    fn container_checks_use_the_explicit_name_and_limit_legacy_inspection() {
        let (_, args) = invocation(Command::ContainerStatus).unwrap();
        assert!(args.contains(&"pullboard"));
        assert!(!args.contains(&"systemd-pullboard"));
        let (_, args) = invocation(Command::LegacyContainerStatus).unwrap();
        assert!(args.contains(&"systemd-pullboard"));
        let (_, args) = invocation(Command::ContainerInventory).unwrap();
        assert!(args.contains(&"--filter=name=^pullboard$"));
    }
    #[test]
    fn commands_never_enable_or_shell_or_linger() {
        for command in [
            Command::PodmanInfo,
            Command::UserManager,
            Command::ServiceStatus,
            Command::GraphicalStatus,
            Command::ContainerStatus,
            Command::LegacyContainerStatus,
            Command::ContainerInventory,
            Command::Pull(Channel::Stable),
            Command::Pull(Channel::Nightly),
            Command::Reload,
            Command::Start,
            Command::Restart,
        ] {
            let (program, args) = invocation(command).unwrap();
            assert!(matches!(program, "podman" | "systemctl"));
            assert!(
                !args
                    .iter()
                    .any(|v| matches!(*v, "enable" | "sudo" | "enable-linger"))
            );
            if program == "systemctl" {
                assert!(args.contains(&"--user"));
            }
        }
    }
    #[test]
    fn source_errors_are_not_exposed_and_unknown_states_fail_closed() {
        let value = serde_json::json!({"state":"idle","sources":[{"source_id":"jira","status":"error","error":"SECRET_TOKEN"}]});
        let parsed = parse_sources(&value).unwrap();
        assert_eq!(parsed[0].status, SourceStatus::Error);
        assert!(!format!("{parsed:?}").contains("SECRET_TOKEN"));
        assert!(parse_sources(&serde_json::json!({"state":"idle","sources":[{"source_id":"jira","status":"partial"}]})).is_none());
    }

    #[tokio::test]
    async fn subprocess_deadlines_cancellation_and_output_limits_are_real() {
        let cancel = CancellationToken::new();
        let begin = std::time::Instant::now();
        let result = run_bounded(
            "/usr/bin/sleep",
            &["30"],
            Duration::from_millis(10),
            &cancel,
        )
        .await;
        assert!(result.err().unwrap().to_string().contains("timed out"));
        assert!(begin.elapsed() < Duration::from_secs(3));

        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            token.cancel();
        });
        let result = run_bounded("/usr/bin/sleep", &["30"], Duration::from_secs(30), &cancel).await;
        task.await.unwrap();
        assert_eq!(result.err(), Some(CANCELLED));

        let result = run_bounded(
            "/usr/bin/head",
            &["-c", "1048578", "/dev/zero"],
            Duration::from_secs(3),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("safe size limit")
        );
    }

    async fn response(
        status: &str,
        headers: &str,
        body: Vec<u8>,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let count = socket.read(&mut request).await.unwrap();
            socket.write_all(head.as_bytes()).await.unwrap();
            let _ = socket.write_all(&body).await;
            String::from_utf8_lossy(&request[..count]).into_owned()
        });
        (port, task)
    }

    #[tokio::test]
    async fn local_readiness_uses_get_contract_not_status_code_or_redirect_success() {
        let probe = HttpProbe::new().unwrap();
        let valid = br#"{"status":"ok","version":"1.0","read_only_test_mode":false,"mode_revision":"current"}"#;
        let (port, request) = response(
            "200 OK",
            &format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                valid.len()
            ),
            valid.to_vec(),
        )
        .await;
        assert!(probe.health(port, &CancellationToken::new()).await.unwrap());
        assert!(
            request
                .await
                .unwrap()
                .starts_with("GET /api/health HTTP/1.1")
        );
        for (status, headers, body) in [
            (
                "200 OK",
                "Content-Length: 15\r\n",
                br#"{"status":"ok"}"#.to_vec(),
            ),
            (
                "302 Found",
                "Location: http://example.invalid/secret\r\nContent-Length: 0\r\n",
                Vec::new(),
            ),
            (
                "500 Failed",
                "Content-Length: 12\r\n",
                b"SECRET_TOKEN".to_vec(),
            ),
        ] {
            let (port, request) = response(status, headers, body).await;
            assert!(!probe.health(port, &CancellationToken::new()).await.unwrap());
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn local_readiness_bounds_streaming_and_announced_response_sizes() {
        let probe = HttpProbe::new().unwrap();
        for headers in [
            format!("Content-Length: {}\r\n", MAX_FILE + 10),
            String::new(),
        ] {
            let (port, request) = response("200 OK", &headers, vec![b'x'; MAX_FILE + 10]).await;
            assert!(!probe.health(port, &CancellationToken::new()).await.unwrap());
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn local_source_summary_retains_health_but_marks_in_progress_refresh() {
        let probe = HttpProbe::new().unwrap();
        let body = serde_json::json!({"state":"syncing","sources":[{"source_id":"jira","status":"ok","error":null},{"source_id":"work","status":"backoff","error":"SECRET_TOKEN"}]}).to_string();
        let (port, request) = response(
            "200 OK",
            &format!("Content-Length: {}\r\n", body.len()),
            body.into_bytes(),
        )
        .await;
        let summary = probe
            .sources(port, &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert!(summary.iter().all(|s| s.refreshing));
        assert_eq!(summary[0].status, SourceStatus::Ok);
        assert_eq!(summary[1].status, SourceStatus::Backoff);
        assert!(!format!("{summary:?}").contains("SECRET_TOKEN"));
        assert!(request.await.unwrap().starts_with("GET /api/sync HTTP/1.1"));
    }

    #[test]
    fn existing_container_port_must_be_exclusively_loopback_owned_and_running() {
        let good = serde_json::json!({"State":{"Running":true},"Config":{"Labels":{"PODMAN_SYSTEMD_UNIT":"pullboard.service"}},"NetworkSettings":{"Ports":{"8080/tcp":[{"HostIp":"127.0.0.1","HostPort":"3333"}]}}});
        assert!(owns_port(&good.to_string(), 3333));
        for (pointer, replacement) in [
            ("/State/Running", serde_json::json!(false)),
            (
                "/Config/Labels/PODMAN_SYSTEMD_UNIT",
                serde_json::json!("other.service"),
            ),
            (
                "/NetworkSettings/Ports/8080~1tcp/0/HostIp",
                serde_json::json!("0.0.0.0"),
            ),
            (
                "/NetworkSettings/Ports/8080~1tcp/0/HostPort",
                serde_json::json!("4444"),
            ),
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(pointer).unwrap() = replacement;
            assert!(!owns_port(&bad.to_string(), 3333));
        }
    }
}
