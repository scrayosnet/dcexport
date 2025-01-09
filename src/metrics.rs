//! This module implements the metrics handler and its http server.

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::routing::get;
use axum::{Extension, Router};
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue, LabelValueEncoder};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{debug, instrument, trace};

/// The prefix ued to all application metrics.
const PREFIX: &str = "dcexport";

/// [Boolean] is a wrapper for [bool] that implements [`EncodeLabelValue`] such that it can be used in
/// metrics labels.
///
/// It encodes [true] as "true" and [false] as "false".
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Boolean(bool);

impl From<Boolean> for bool {
    fn from(val: Boolean) -> Self {
        val.0
    }
}

impl From<bool> for Boolean {
    fn from(val: bool) -> Self {
        Boolean(val)
    }
}

impl EncodeLabelValue for Boolean {
    fn encode(&self, encoder: &mut LabelValueEncoder) -> Result<(), std::fmt::Error> {
        if self.0 {
            "true".encode(encoder)
        } else {
            "false".encode(encoder)
        }
    }
}

/// [`GuildsLabels`] are the [labels](EncodeLabelSet) for the `guild` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct GuildsLabels {
    pub guild_id: u64,
}

/// [`ChannelLabels`] are the [labels](EncodeLabelSet) for the `channel` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ChannelLabels {
    pub guild_id: u64,
    pub channel_id: u64,
    pub channel_name: String,
    pub channel_nsfw: Boolean,
    pub channel_type: String,
}

/// [`BoostLabels`] are the [labels](EncodeLabelSet) for the `boost` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct BoostLabels {
    pub guild_id: u64,
}

/// [`MemberLabels`] are the [labels](EncodeLabelSet) for the `member` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberLabels {
    pub guild_id: u64,
}

/// [`BotLabels`] are the [labels](EncodeLabelSet) for the `bot` metric.
///
/// This metric is not included in the member metric (using a label) as the user bot status has to
/// be explicitly requested on guild creation. As such, they are separated to ensure that the member
/// metric does not suffer from additional requests (that could potentially fail).
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct BotLabels {
    pub guild_id: u64,
}

/// [`MemberStatusLabels`] are the [labels](EncodeLabelSet) for the `member_status` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberStatusLabels {
    pub guild_id: u64,
    pub status: String,
}

/// [`MemberVoiceLabels`] are the [labels](EncodeLabelSet) for the `member_voice` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberVoiceLabels {
    pub guild_id: u64,
    pub category_id: Option<u64>,
    pub channel_id: u64,
    pub self_stream: Boolean,
    pub self_video: Boolean,
    pub self_deaf: Boolean,
    pub self_mute: Boolean,
}

/// [`MessageSentLabels`] are the [labels](EncodeLabelSet) for the `message_sent` metric.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MessageSentLabels {
    pub guild_id: u64,
    pub category_id: Option<u64>,
    pub channel_id: u64,
}

/// [`EmoteUsedLabels`] are the [labels](EncodeLabelSet) for the `emote_used` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct EmoteUsedLabels {
    pub guild_id: u64,
    pub category_id: Option<u64>,
    pub channel_id: u64,
    pub reaction: Boolean,
    pub emoji_id: u64,
    pub emoji_name: Option<String>,
}

/// [`ActivityLabels`] are the [labels](EncodeLabelSet) for the `activity` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ActivityLabels {
    pub guild_id: u64,
    pub activity_application_id: Option<u64>,
    pub activity_name: String,
}

/// Handler is the [servable](serve) bundle of metrics for the exporter.
pub struct Handler {
    registry: Registry,
    pub guild: Family<GuildsLabels, Gauge>,
    pub channel: Family<ChannelLabels, Gauge>,
    pub boost: Family<BoostLabels, Gauge>,
    pub member: Family<MemberLabels, Gauge>,
    pub bot: Family<BotLabels, Gauge>,
    pub member_status: Family<MemberStatusLabels, Gauge>,
    pub member_voice: Family<MemberVoiceLabels, Gauge>,
    pub message_sent: Family<MessageSentLabels, Counter>,
    pub emote_used: Family<EmoteUsedLabels, Counter>,
    pub activity: Family<ActivityLabels, Gauge>,
}

impl Handler {
    /// Creates a new [Handler] metrics bundle with its own [Registry].
    ///
    /// The [Registry] is created using a [PREFIX].
    #[instrument]
    pub fn new() -> Self {
        debug!(prefix = PREFIX, "Building metrics registry");
        let mut registry = <Registry>::with_prefix(PREFIX);

        debug!(metrics_name = "guild", "Building metric");
        let guild = Family::<GuildsLabels, Gauge>::default();
        registry.register(
            "guild",
            "The number of guilds handled by the exporter.",
            guild.clone(),
        );

        debug!(metrics_name = "channel", "Building metric");
        let channel = Family::<ChannelLabels, Gauge>::default();
        registry.register(
            "channel",
            "The number of channels on the guild.",
            channel.clone(),
        );

        debug!(metrics_name = "boost", "Building metric");
        let boost = Family::<BoostLabels, Gauge>::default();
        registry.register(
            "boost",
            "The number of boosts active on the guild.",
            boost.clone(),
        );

        debug!(metrics_name = "member", "Building metric");
        let member = Family::<MemberLabels, Gauge>::default();
        registry.register(
            "member",
            "The number of members (including bots) on the guild.",
            member.clone(),
        );

        debug!(metrics_name = "bot", "Building metric");
        let bot = Family::<BotLabels, Gauge>::default();
        registry.register(
            "bot",
            "The number of bot members on the guild.",
            bot.clone(),
        );

        debug!(metrics_name = "member_status", "Building metric");
        let member_status = Family::<MemberStatusLabels, Gauge>::default();
        registry.register(
            "member_status",
            "The number of members on the guild per status.",
            member_status.clone(),
        );

        debug!(metrics_name = "member_voice", "Building metric");
        let member_voice = Family::<MemberVoiceLabels, Gauge>::default();
        registry.register(
            "member_voice",
            "The number of members in voice channels.",
            member_voice.clone(),
        );

        debug!(metrics_name = "message_sent", "Building metric");
        let message_sent = Family::<MessageSentLabels, Counter>::default();
        registry.register(
            "message_sent",
            "The total number of discord messages sent by guild members.",
            message_sent.clone(),
        );

        debug!(metrics_name = "emote_used", "Building metric");
        let emote_used = Family::<EmoteUsedLabels, Counter>::default();
        registry.register(
            "emote_used",
            "The total number of discord emotes reacted with by guild members in messages.",
            emote_used.clone(),
        );

        debug!(metrics_name = "activity", "Building metric");
        let activity = Family::<ActivityLabels, Gauge>::default();
        registry.register(
            "activity",
            "The number of current activities.",
            activity.clone(),
        );

        Self {
            registry,
            // metrics
            guild,
            channel,
            boost,
            member,
            bot,
            member_status,
            member_voice,
            message_sent,
            emote_used,
            activity,
        }
    }
}

/// Serves a shared [Handler] using a [webserver](Router).
///
/// Use the [CancellationToken] to cancel and gracefully shutdown the [Handler].
/// The metrics can be accessed using the `/metrics` path. It doesn't enforce any authentication.
#[instrument(skip(handler, shutdown))]
pub async fn serve(
    address: &SocketAddr,
    handler: Arc<Handler>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create webserver for metrics
    let rest_app = Router::new()
        .route("/", get(index))
        .route("/metrics", get(metrics))
        .layer(Extension(Arc::clone(&handler)))
        .layer(TraceLayer::new_for_http())
        .with_state(());

    // Bind tcp listener
    debug!(address = %address, "Starting tcp listener");
    let listener = tokio::net::TcpListener::bind(address).await?;

    // Serve webserver and wait
    debug!("Serving axum router");
    axum::serve(listener, rest_app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;

    Ok(())
}

/// The index endpoint handler. It shows an index page.
#[instrument]
async fn index() -> Html<&'static str> {
    Html("dcexport - <a href=\"/metrics\">Metrics</a>")
}

/// The metrics endpoint handler. It encodes the current registry into the response body.
///
/// The body has the [CONTENT_TYPE] `application/openmetrics-text; version=1.0.0; charset=utf-8`.
#[instrument(skip(handler))]
async fn metrics(Extension(handler): Extension<Arc<Handler>>) -> Response {
    debug!("Handling metrics request");

    // Encode the metrics content into the buffer
    let mut buffer = String::new();
    encode(&mut buffer, &handler.registry).expect("failed to encode metrics into the buffer");
    trace!(buffer = buffer.to_string(), "Built metrics response");

    // Respond with encoded metrics
    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )
        .body(Body::from(buffer))
        .expect("failed to build response")
}
