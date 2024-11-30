use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue, LabelValueEncoder};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{debug, instrument, trace};

/// The prefix ued to all application metrics.
const PREFIX: &str = "dcexport";

/// The address (port) of the application metrics.
const ADDRESS: &str = "0.0.0.0:8080";

/// [Boolean] is a wrapper for [bool] that implements [EncodeLabelValue] such that it can be used in
/// metrics labels.
///
/// It encodes [true] as "true" and [false] as "false".
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct Boolean(bool);

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
        match self.0 {
            true => "true".encode(encoder),
            false => "false".encode(encoder),
        }
    }
}

/// [GuildsLabels] are the [labels](EncodeLabelSet) for the "guild" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct GuildsLabels {
    pub(crate) guild_id: u64,
}

/// [MessageSentLabels] are the [labels](EncodeLabelSet) for the "message_sent" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MessageSentLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
}

/// [EmoteUsedLabels] are the [labels](EncodeLabelSet) for the "emote_used" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct EmoteUsedLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
    pub(crate) reaction: Boolean,
    pub(crate) emoji_id: u64,
    pub(crate) emoji_name: Option<String>,
}

/// [ActivityLabels] are the [labels](EncodeLabelSet) for the "activity" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct ActivityLabels {
    pub(crate) guild_id: u64,
    pub(crate) activity_application_id: Option<u64>,
    pub(crate) activity_name: String,
}

/// [MemberLabels] are the [labels](EncodeLabelSet) for the "member" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MemberLabels {
    pub(crate) guild_id: u64,
}

/// [MemberStatusLabels] are the [labels](EncodeLabelSet) for the "member_status" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MemberStatusLabels {
    pub(crate) guild_id: u64,
    pub(crate) status: String,
}

/// [MemberVoiceLabels] are the [labels](EncodeLabelSet) for the "member_voice" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MemberVoiceLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
    pub(crate) self_stream: Boolean,
    pub(crate) self_video: Boolean,
    pub(crate) self_deaf: Boolean,
    pub(crate) self_mute: Boolean,
}

/// [BoostLabels] are the [labels](EncodeLabelSet) for the "boost" metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct BoostLabels {
    pub(crate) guild_id: u64,
}

/// Handler is the [servable](serve) bundle of metrics for the exporter.
pub(crate) struct Handler {
    registry: Registry,
    pub(crate) guilds: Family<GuildsLabels, Gauge>,
    pub(crate) message_sent: Family<MessageSentLabels, Counter>,
    pub(crate) member: Family<MemberLabels, Gauge>,
    pub(crate) emote_used: Family<EmoteUsedLabels, Counter>,
    pub(crate) activity: Family<ActivityLabels, Gauge>,
    pub(crate) member_status: Family<MemberStatusLabels, Gauge>,
    pub(crate) member_voice: Family<MemberVoiceLabels, Gauge>,
    pub(crate) boost: Family<BoostLabels, Gauge>,
}

impl Handler {
    /// Creates a new [Handler] metrics bundle with its own [Registry].
    ///
    /// The [Registry] is created using a [PREFIX].
    #[instrument]
    pub(crate) fn new() -> Self {
        debug!(prefix = PREFIX, "Building metrics registry");
        let mut registry = <Registry>::with_prefix(PREFIX);

        debug!(metrics_name = "guilds", "Building metric");
        let guilds = Family::<GuildsLabels, Gauge>::default();
        registry.register("guilds", "The total number of guilds.", guilds.clone());

        debug!(metrics_name = "message_sent", "Building metric");
        let message_sent = Family::<MessageSentLabels, Counter>::default();
        registry.register(
            "message_sent",
            "The total number of discord messages sent by users.",
            message_sent.clone(),
        );

        debug!(metrics_name = "emote_used", "Building metric");
        let emote_used = Family::<EmoteUsedLabels, Counter>::default();
        registry.register(
            "emote_used",
            "The total number of discord emotes sent by users in messages.",
            emote_used.clone(),
        );

        debug!(metrics_name = "emote_used", "Building metric");
        let emote_used = Family::<EmoteUsedLabels, Counter>::default();
        registry.register(
            "emote_used",
            "The total number of discord emotes reacted with by users in messages.",
            emote_used.clone(),
        );

        debug!(metrics_name = "activity", "Building metric");
        let activity = Family::<ActivityLabels, Gauge>::default();
        registry.register(
            "activity",
            "The total number of current activities.",
            activity.clone(),
        );

        debug!(metrics_name = "member", "Building metric");
        let member = Family::<MemberLabels, Gauge>::default();
        registry.register(
            "member",
            "The total number of members on the guild.",
            member.clone(),
        );

        debug!(metrics_name = "member_status", "Building metric");
        let member_status = Family::<MemberStatusLabels, Gauge>::default();
        registry.register(
            "member_status",
            "The total number of members on the guild per status.",
            member_status.clone(),
        );

        debug!(metrics_name = "member_voice", "Building metric");
        let member_voice = Family::<MemberVoiceLabels, Gauge>::default();
        registry.register(
            "member_voice",
            "The total number of members in voice channels.",
            member_voice.clone(),
        );

        debug!(metrics_name = "boost", "Building metric");
        let boost = Family::<BoostLabels, Gauge>::default();
        registry.register(
            "boost",
            "The total number of boosts on the guild.",
            boost.clone(),
        );

        Self {
            registry,
            // metrics
            guilds,
            message_sent,
            emote_used,
            activity,
            member,
            member_status,
            member_voice,
            boost,
        }
    }
}

/// Serves a shared [Handler] using a [webserver](Router).
///
/// Use the [CancellationToken] to cancel and gracefully shutdown the [Handler].
/// The metrics can be accessed using the `/metrics` path. It doesn't enforce any authentication.
#[instrument(skip(handler, shutdown))]
pub(crate) async fn serve(
    handler: Arc<Handler>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create webserver for metrics
    let rest_app = Router::new()
        .route("/metrics", get(metrics))
        .layer(Extension(Arc::clone(&handler)))
        .layer(TraceLayer::new_for_http())
        .with_state(());

    // Bind tcp listener
    debug!(address = ADDRESS, "Starting tcp listener");
    let listener = tokio::net::TcpListener::bind(ADDRESS).await?;

    // Serve webserver and wait
    debug!("Serving axum router");
    axum::serve(listener, rest_app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;

    Ok(())
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
