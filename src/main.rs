use clap::Parser;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::str::FromStr;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// The default address (port) of the application metrics server.
pub const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";

/// The default log level of the application.
pub const DEFAULT_LOG: &str = "info";

/// [`Log`] is a wrapper for [`EnvFilter`] such that it implements [`Clone`]. This is required to be a clap arg.
#[derive(Debug)]
struct Log(EnvFilter);

impl Display for Log {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Clone for Log {
    fn clone(&self) -> Self {
        Log(EnvFilter::from(&self.0.to_string()))
    }
}

impl FromStr for Log {
    type Err = tracing_subscriber::filter::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Log(EnvFilter::try_new(s)?))
    }
}

/// Arguments to configure this runtime of the application before it is started.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, env = "DCEXPORT_DISCORD_TOKEN")]
    discord_token: String,
    #[arg(long, env = "DCEXPORT_LOG", default_value = DEFAULT_LOG, value_parser = clap::value_parser!(Log))]
    log: Log,
    #[arg(long, env = "DCEXPORT_ADDRESS", default_value = DEFAULT_ADDRESS)]
    address: SocketAddr,
}

/// Initializes the application and invokes dcexport.
///
/// This initializes the logging, aggregates configuration and starts the multithreaded tokio runtime. This is only a
/// thin-wrapper around the dcexport crate that supplies the necessary settings. The application also implements a
/// graceful shutdown procedure that will stop the subtasks and wait for them to finish.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse the arguments and configuration
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact())
        .with(args.log.0)
        .init();

    // Run dcexport blocking
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { dcexport::start(args.address, args.discord_token).await })
}
