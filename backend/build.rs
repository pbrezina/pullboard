#![forbid(unsafe_code)]

use std::{env, path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn main() {
    println!("cargo::rerun-if-env-changed=PULLBOARD_BUILD_VERSION");
    println!("cargo::rerun-if-changed=build.rs");

    // Container builders receive this non-secret metadata from the checkout.
    let supplied = env::var("PULLBOARD_BUILD_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let version = supplied.unwrap_or_else(|| {
        // Resolve paths through Git to support both normal clones and worktrees.
        for name in ["HEAD", "refs", "packed-refs", "shallow"] {
            if let Some(path) = git(&["rev-parse", "--git-path", name])
                && Path::new(&path).exists()
            {
                println!("cargo::rerun-if-changed={path}");
            }
        }
        // Ignore the moving `nightly` tag; without a release tag, show the commit.
        git(&[
            "describe",
            "--tags",
            "--match",
            "v[0-9]*",
            "--always",
            "--abbrev=12",
        ])
        .unwrap_or_else(|| format!("{}+unknown", env!("CARGO_PKG_VERSION")))
    });
    assert!(
        !version.contains(['\r', '\n']),
        "build version must be a single line"
    );
    println!("cargo::rustc-env=PULLBOARD_BUILD_VERSION={version}");
}
