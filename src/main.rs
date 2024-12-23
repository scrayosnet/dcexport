mod discord;
mod metrics;

use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::metadata::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// The default logging level.
///
/// The string has to be deserializable using [LevelFilter::from_str]. In general, the info logging
/// level should be ideal for staging and production environments. As such, the corresponding env
/// variable should generally be left empty.
const DEFAULT_LOGGING_LEVEL: &str = "info";

/// The public response error wrapper for all errors that can be relayed to the caller.
#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("missing discord token")]
    MissingDiscordToken(#[from] env::VarError),
    #[error("failed to parse logging level")]
    InvalidFilter(#[from] tracing::metadata::ParseLevelFilterError),
}

/// Initializes the application and starts the Discord bot and metrics server.
///
/// The settings are initially read and frozen, any future changes on e.g. the environment variables
/// will not change the application configuration. The application also implements a graceful shutdown
/// procedure that will stop the subtasks and wait for them to finish.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read configuration from the environment variables
    let discord_token = env::var("DCEXPORT_DISCORD_TOKEN").map_err(Error::MissingDiscordToken)?;
    let logging_level =
        env::var("DCEXPORT_LOGGING_LEVEL").unwrap_or(DEFAULT_LOGGING_LEVEL.to_string());

    // Initialize logging with sentry hook
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_filter(LevelFilter::from_str(&logging_level).map_err(Error::InvalidFilter)?),
        )
        .init();

    // Create metrics handler
    let metrics_handler = Arc::new(metrics::Handler::new());

    // Create discord handler
    let discord_handler = discord::Handler::new(Arc::clone(&metrics_handler));

    // Create tracker and cancellation token, they are used to implement a graceful shutdown for the handlers
    let tracker = TaskTracker::new();
    let token = CancellationToken::new();

    // Start discord handler
    {
        // Shadow tracker and token for move
        let tracker = tracker.clone();
        let token = token.clone();
        // Spawn task in tracker
        tracker.clone().spawn(async move {
            info!("Starting discord handler");
            if let Err(why) = discord::serve(&discord_token, discord_handler, token.clone()).await {
                error!(err = why, "Discord handler aborted");
            }
            info!("Stopped discord handler");
            tracker.close();
            token.cancel();
        });
    }

    // Start metrics handler
    {
        // Shadow tracker and token for move
        let tracker = tracker.clone();
        let token = token.clone();
        // Spawn task in tracker
        tracker.clone().spawn(async move {
            info!("Starting metrics handler");
            if let Err(why) = metrics::serve(metrics_handler, token.clone()).await {
                error!(err = why, "Metrics handler aborted");
            }
            info!("Stopped metrics handler");
            tracker.close();
            token.cancel();
        });
    }

    // Listen for system shutdown signal (in main thread)
    info!("Listening for signal received");
    select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
        // Explicitly wait for token cancellation such that errors from the handlers
        // result in an application shutdown
        () = token.cancelled() => {
            info!("System shutdown before shutdown signal received");
        }
    }
    tracker.close();
    token.cancel();

    // Wait for all tasks to finish (graceful shutdown)
    tracker.wait().await;

    Ok(())
}
