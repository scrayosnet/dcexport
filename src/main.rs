mod discord;
mod metrics;
mod settings;

use crate::settings::Settings;
use std::sync::Arc;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read settings from config files and environment variables
    let settings = Arc::new(Settings::new()?);

    // initialize logging with sentry hook
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_filter(settings.logging.level),
        )
        .init();

    // Create metrics handler
    let metrics_handler = Arc::new(metrics::Handler::new(settings.metrics.clone()));

    // Create discord handler
    let discord_handler =
        discord::Handler::new(settings.discord.clone(), Arc::clone(&metrics_handler));

    // Create tracker and cancellation token, they are used to implement a graceful shutdown for the handlers
    let tracker = TaskTracker::new();
    let token = CancellationToken::new();

    // Start discord handler
    {
        // Shadow tracker and token for move
        let tracker = tracker.clone();
        let token = token.clone();
        // Spawn task in tracker to awaitable
        tracker.clone().spawn(async move {
            info!("Starting discord handler");
            if let Err(why) = discord::serve(discord_handler, token.clone()).await {
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
        // Spawn task in tracker to awaitable
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

    // Listen for system shutdown signal
    {
        // Shadow tracker and token for move
        let tracker = tracker.clone();
        let token = token.clone();
        // Spawn task in tracker to awaitable
        tracker.clone().spawn(async move {
            info!("Listening for signal received");
            select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutdown signal received");
                }
                () = token.cancelled() => {}
            }
            tracker.close();
            token.cancel();
        });
    }

    // Wait for all tasks to finish (graceful shutdown)
    tracker.wait().await;

    Ok(())
}
