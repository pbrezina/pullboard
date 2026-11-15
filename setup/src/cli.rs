use clap::Parser;

/// Tokens are entered interactively only; no credential arguments or environment inputs.
#[derive(Parser, Debug)]
#[command(
    name = "pullboard-setup",
    version,
    about = "Configure and install PullBoard with rootless Podman (Linux x86-64)"
)]
pub struct Cli {
    /// Prefill the editable Jira Cloud tenant URL; does not authorize a network request.
    #[arg(long, value_parser = jira_cloud_url)]
    pub jira_cloud_url: Option<String>,
    /// Preselect the Nightly container image; editable in Runtime.
    #[arg(long)]
    pub nightly: bool,
    /// Use terminal defaults without color (also honors NO_COLOR).
    #[arg(long)]
    pub no_color: bool,
}

/// Only direct HTTPS Atlassian tenant roots, never the scoped-token API gateway.
pub fn jira_cloud_url(value: &str) -> Result<String, String> {
    let error = || {
        "use an HTTPS Jira Cloud tenant root, without credentials, path, query or fragment"
            .to_owned()
    };
    if value.contains(['\\', '%'])
        || value
            .strip_prefix("https://")
            .is_none_or(|s| s.trim_end_matches('/').contains('/'))
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
        ] {
            assert!(jira_cloud_url(s).is_err(), "{s}");
        }
    }
}
