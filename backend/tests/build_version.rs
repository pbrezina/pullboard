use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};
use tempfile::TempDir;

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn build_script() -> &'static Path {
    static SCRIPT: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &SCRIPT
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let binary = directory.path().join("build-version");
            success(
                Command::new("rustc")
                    .args(["--edition=2024", "-Dwarnings"])
                    .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"))
                    .arg("-o")
                    .arg(&binary)
                    .env("CARGO_PKG_VERSION", "9.9.9")
                    .output()
                    .unwrap(),
            );
            (directory, binary)
        })
        .1
}

fn git(directory: &Path, args: &[&str]) -> String {
    success(
        Command::new("git")
            .args([
                "-c",
                "user.name=Build version test",
                "-c",
                "user.email=build@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap(),
    )
}

fn repository() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "--quiet"]);
    git(
        directory.path(),
        &["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
    directory
}

fn version(directory: &Path, supplied: Option<&str>) -> String {
    let mut command = Command::new(build_script());
    command
        .current_dir(directory)
        .env_remove("PULLBOARD_BUILD_VERSION");
    if let Some(value) = supplied {
        command.env("PULLBOARD_BUILD_VERSION", value);
    }
    let output = success(command.output().unwrap());
    output
        .lines()
        .find_map(|line| line.strip_prefix("cargo::rustc-env=PULLBOARD_BUILD_VERSION="))
        .unwrap()
        .to_owned()
}

#[test]
fn untagged_and_rolling_nightly_builds_show_the_commit() {
    let directory = repository();
    let path = directory.path();
    let commit = git(path, &["rev-parse", "--short=12", "HEAD"]);
    assert_eq!(version(path, None), commit);
    git(path, &["tag", "nightly"]);
    assert_eq!(version(path, None), commit);
}

#[test]
fn release_tags_and_subsequent_commits_are_described() {
    let directory = repository();
    let path = directory.path();
    git(path, &["tag", "v1.2.3"]);
    assert_eq!(version(path, None), "v1.2.3");
    git(path, &["commit", "--quiet", "--allow-empty", "-m", "next"]);
    git(path, &["tag", "nightly"]);
    let commit = git(path, &["rev-parse", "--short=12", "HEAD"]);
    assert_eq!(version(path, None), format!("v1.2.3-1-g{commit}"));
}

#[test]
fn supplied_container_metadata_takes_precedence_over_local_git() {
    let directory = repository();
    assert_eq!(
        version(directory.path(), Some("v2.0.0-7-g0123456789ab")),
        "v2.0.0-7-g0123456789ab"
    );
}

#[test]
fn source_archives_identify_unknown_provenance_unless_metadata_is_supplied() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(version(directory.path(), None), "9.9.9+unknown");
    assert_eq!(version(directory.path(), Some("")), "9.9.9+unknown");
    assert_eq!(version(directory.path(), Some("v1.2.3")), "v1.2.3");
}
