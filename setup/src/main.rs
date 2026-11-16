#![forbid(unsafe_code)]
use clap::Parser;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = match pullboard_setup::cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // Clap's normal diagnostics include rejected argument values, which
            // may contain accidentally supplied credentials. Only static help
            // and version output may use its renderer.
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return std::process::ExitCode::SUCCESS;
            }
            eprintln!(
                "PullBoard setup: invalid command-line arguments; use --help for usage.\nJira URLs must be HTTPS tenant roots without credentials, paths, query or fragment. Enter tokens only inside the wizard."
            );
            return std::process::ExitCode::from(2);
        }
    };
    if cli.jira.validate().is_err() {
        eprintln!(
            "PullBoard setup: use 1..100 unique ordered board IDs and sprint patterns; My board and My pattern go first."
        );
        return std::process::ExitCode::from(2);
    }
    match pullboard_setup::wizard::run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "PullBoard setup: {}",
                pullboard_setup::prompt::clean(&error.to_string())
            );
            std::process::ExitCode::FAILURE
        }
    }
}
