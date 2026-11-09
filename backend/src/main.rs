#![forbid(unsafe_code)]
use clap::Parser;
use pullboard::{
    api::{App, router},
    config::Config,
};
use std::{net::SocketAddr, path::PathBuf};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{Layer, filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(
    version,
    about = "Local PullBoard server (one YAML configuration file)"
)]
struct Cli {
    #[arg(long, default_value = "config/pullboard.yaml")]
    config: PathBuf,
    #[arg(long)]
    check_config: bool,
}

#[tokio::main]
async fn main() {
    // Do not enable dependency HTTP/credential logs via environment variables.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_filter(
                    Targets::new()
                        .with_default(tracing::Level::WARN)
                        .with_target("pullboard", tracing::Level::INFO),
                ),
        )
        .init();
    if let Err(error) = run().await {
        eprintln!("PullBoard: {error}");
        std::process::exit(1);
    }
}
async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config).await?;
    if cli.check_config {
        config.check_secrets().await?;
        println!("Configuration and configured credential sources are valid.");
        return Ok(());
    }
    let address = SocketAddr::new(config.server.bind, config.server.port);
    let app = App::build(config).await?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| anyhow::anyhow!("could not bind the configured local listening address"))?;
    let cancel = CancellationToken::new();
    let worker = app.sync.spawn(cancel.clone());
    let shutdown = cancel.clone();
    let server = axum::serve(listener, router(app))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                if let Ok(mut terminate) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{}}
                } else {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            shutdown.cancel();
        })
        .await;
    cancel.cancel();
    let _ = worker.await;
    server.map_err(|_| anyhow::anyhow!("local HTTP server failed"))
}
