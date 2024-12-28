//! A Discord guild Prometheus exporter. This application uses a Discord bot to track multiple Discord guilds.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

mod discord;
mod metrics;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};

/// Starts the application discord listener and metrics server. The application also implements a graceful shutdown
/// procedure that will stop the subtasks and wait for them to finish.
///
/// # Errors
///
/// Currently, no error is returned, only logged.
pub async fn start(
    address: SocketAddr,
    discord_token: String,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create metrics handler
    let metrics_handler = Arc::new(metrics::Handler::new());

    // Create discord handler (wrapping the metrics handler)
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
            if let Err(why) = metrics::serve(&address, metrics_handler, token.clone()).await {
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
            warn!("System shutdown before shutdown signal received");
        }
    }
    tracker.close();
    token.cancel();

    // Wait for all tasks to finish (graceful shutdown)
    tracker.wait().await;
    info!("Shutdown successfully");

    Ok(())
}
