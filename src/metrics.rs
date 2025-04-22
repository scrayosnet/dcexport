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
use serenity::all::{
    Activity, ApplicationId, ChannelId, EmojiId, Guild, GuildChannel, OnlineStatus, VoiceState,
};
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
    pub guild_name: String,
}

impl GuildsLabels {
    /// Creates a new instance of [`GuildsLabels`].
    pub fn new(guild: &Guild) -> Self {
        Self {
            guild_name: guild.name.clone(),
        }
    }
}

/// [`ChannelLabels`] are the [labels](EncodeLabelSet) for the `channel` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ChannelLabels {
    pub channel_id: u64,
    pub channel_name: String,
    pub channel_nsfw: Boolean,
    pub channel_type: String,
}

impl ChannelLabels {
    /// Creates a new instance of [`ChannelLabels`].
    pub fn new(channel: &GuildChannel) -> Self {
        Self {
            channel_id: channel.id.get(),
            channel_name: channel.name.clone(),
            channel_nsfw: Boolean(channel.nsfw),
            channel_type: channel.kind.name().to_string(),
        }
    }
}

/// [`BoostLabels`] are the [labels](EncodeLabelSet) for the `boost` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct BoostLabels {}

impl BoostLabels {
    /// Creates a new instance of [`BoostLabels`].
    pub fn new() -> Self {
        Self {}
    }
}

/// [`MemberLabels`] are the [labels](EncodeLabelSet) for the `member` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberLabels {}

impl MemberLabels {
    /// Creates a new instance of [`MemberLabels`].
    pub fn new() -> Self {
        Self {}
    }
}

/// [`BotLabels`] are the [labels](EncodeLabelSet) for the `bot` metric.
///
/// This metric is not included in the member metric (using a label) as the user bot status has to
/// be explicitly requested on guild creation. As such, they are separated to ensure that the member
/// metric does not suffer from additional requests (that could potentially fail).
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct BotLabels {}

impl BotLabels {
    /// Creates a new instance of [`BotLabels`].
    pub fn new() -> Self {
        Self {}
    }
}

/// [`MemberStatusLabels`] are the [labels](EncodeLabelSet) for the `member_status` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberStatusLabels {
    pub status: String,
}

impl MemberStatusLabels {
    /// Creates a new instance of [`MemberStatusLabels`].
    pub fn new(status: OnlineStatus) -> Self {
        Self {
            status: status.name().to_string(),
        }
    }
}

/// [`MemberVoiceLabels`] are the [labels](EncodeLabelSet) for the `member_voice` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MemberVoiceLabels {
    pub category_id: Option<u64>,
    pub channel_id: u64,
    pub self_stream: Boolean,
    pub self_video: Boolean,
    pub self_deaf: Boolean,
    pub self_mute: Boolean,
}

impl MemberVoiceLabels {
    /// Creates a new instance of [`MemberVoiceLabels`].
    pub fn new(category_id: Option<ChannelId>, channel_id: ChannelId, voice: &VoiceState) -> Self {
        Self {
            category_id: category_id.map(ChannelId::get),
            channel_id: channel_id.get(),
            self_stream: voice.self_stream.unwrap_or(false).into(),
            self_video: voice.self_video.into(),
            self_deaf: voice.self_deaf.into(),
            self_mute: voice.self_mute.into(),
        }
    }
}

/// [`MessageSentLabels`] are the [labels](EncodeLabelSet) for the `message_sent` metric.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MessageSentLabels {
    pub category_id: Option<u64>,
    pub channel_id: u64,
}

impl MessageSentLabels {
    /// Creates a new instance of [`MessageSentLabels`].
    pub fn new(category_id: Option<ChannelId>, channel_id: ChannelId) -> Self {
        Self {
            category_id: category_id.map(ChannelId::get),
            channel_id: channel_id.get(),
        }
    }
}

/// [`EmoteUsedLabels`] are the [labels](EncodeLabelSet) for the `emote_used` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct EmoteUsedLabels {
    pub category_id: Option<u64>,
    pub channel_id: u64,
    pub reaction: Boolean,
    pub emoji_id: u64,
    pub emoji_name: Option<String>,
}

impl EmoteUsedLabels {
    /// Creates a new instance of [`EmoteUsedLabels`].
    pub fn new(
        category_id: Option<ChannelId>,
        channel_id: ChannelId,
        reaction: bool,
        emoji_id: EmojiId,
        emoji_name: Option<String>,
    ) -> Self {
        Self {
            category_id: category_id.map(ChannelId::get),
            channel_id: channel_id.get(),
            reaction: Boolean(reaction),
            emoji_id: emoji_id.get(),
            emoji_name,
        }
    }
}

/// [`ActivityLabels`] are the [labels](EncodeLabelSet) for the `activity` metric.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ActivityLabels {
    pub activity_application_id: Option<u64>,
    pub activity_name: String,
}

impl ActivityLabels {
    /// Creates a new instance of [`ActivityLabels`].
    pub fn new(activity: &Activity) -> Self {
        Self {
            activity_application_id: activity.application_id.map(ApplicationId::get),
            activity_name: activity.name.clone(),
        }
    }
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

    pub fn clear(&self) {
        self.guild.clear();
        self.channel.clear();
        self.boost.clear();
        self.member.clear();
        self.bot.clear();
        self.member_status.clear();
        self.member_voice.clear();
        self.message_sent.clear();
        self.emote_used.clear();
        self.activity.clear();
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
