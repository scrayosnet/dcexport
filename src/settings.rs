use config::{Config, ConfigError, Environment, File, FileFormat};
use serde::de::{Unexpected, Visitor};
use serde::{Deserialize, Deserializer};
use std::net::SocketAddr;
use std::str::FromStr;
use std::{env, fmt};
use tracing_subscriber::filter::LevelFilter;

/// [Metrics] holds the metrics service configuration. The metrics service is part of the rest server.
/// The rest server will be, if not already so, implicitly enabled if the metrics service is enabled.
/// If enabled, it is exposed at the rest server at `/metrics`.
///
/// Metrics will always be aggregated by the application. This option is only used to expose the metrics
/// service. The service supports basic auth that can be enabled. Make sure to override the default
/// username and password in that case.
#[derive(Debug, Clone, Deserialize)]
pub struct Metrics {
    /// The basic auth username. Override default configuration if basic auth is enabled.
    pub username: String,

    /// The basic auth password. Override default configuration if basic auth is enabled.
    pub password: String,

    pub address: SocketAddr,

    pub prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Discord {
    pub token: String,
}

/// [Logging] hold the log configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    /// The log level that should be printed.
    #[serde(deserialize_with = "parse_level_filter")]
    pub level: LevelFilter,
}

/// [Settings] holds all configuration for the application. I.g. one immutable instance is created
/// on startup and then shared among the application components.
///
/// If both the grpc and rest server are disabled, the application will exit immediately after startup
/// with status ok.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub discord: Discord,

    /// The metrics configuration. The metrics service is part of the [RestServer].
    pub metrics: Metrics,

    pub logging: Logging,
}

impl Settings {
    /// Creates a new application configuration as described in the [module documentation](crate::settings).
    pub fn new() -> Result<Self, ConfigError> {
        // the environment prefix for all `Settings` fields
        let env_prefix = env::var("ENV_PREFIX").unwrap_or("xenos".into());
        // the path of the custom configuration file
        let config_file = env::var("CONFIG_FILE").unwrap_or("config/config".into());

        let s = Config::builder()
            // load default configuration (embedded at compile time)
            .add_source(File::from_str(
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/config/default.toml")),
                FileFormat::Toml,
            ))
            // load custom configuration from file (at runtime)
            .add_source(File::with_name(&config_file).required(false))
            // add in settings from the environment (with a prefix of APP)
            // e.g. `XENOS__DEBUG=1` would set the `debug` key, on the other hand,
            // `XENOS__CACHE__REDIS__ENABLED=1` would enable the redis cache.
            .add_source(Environment::with_prefix(&env_prefix).separator("__"))
            .build()?;

        // you can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
    }
}

impl Default for Settings {
    fn default() -> Self {
        let s = Config::builder()
            // load default configuration (embedded at compile time)
            .add_source(File::from_str(
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/config/default.toml")),
                FileFormat::Toml,
            ))
            .build()
            .expect("expected default configuration to be available");

        // you can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
            .expect("expected default configuration to be deserializable")
    }
}

/// Deserializer for [LevelFilter] from string. E.g. `info`.
pub fn parse_level_filter<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    struct LevelFilterVisitor;

    impl Visitor<'_> for LevelFilterVisitor {
        type Value = LevelFilter;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a log level name or number")
        }

        fn visit_str<E>(self, value: &str) -> Result<LevelFilter, E>
        where
            E: serde::de::Error,
        {
            match LevelFilter::from_str(value) {
                Ok(filter) => Ok(filter),
                Err(_) => Err(serde::de::Error::invalid_value(
                    Unexpected::Str(value),
                    &"log level string or number",
                )),
            }
        }
    }

    deserializer.deserialize_str(LevelFilterVisitor)
}
