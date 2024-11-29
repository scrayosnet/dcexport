use crate::settings;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use axum_auth::AuthBasic;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue, LabelValueEncoder};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

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

// TODO only track custom emojis
// TODO don't track emoji guild id
// TODO don't differ between user, system and bot

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct GuildsLabels {
    pub(crate) guild_id: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MessageSentLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct EmoteSentLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
    pub(crate) emoji_id: u64,
    pub(crate) emoji_name: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct EmoteReactedLabels {
    pub(crate) guild_id: u64,
    pub(crate) category_id: Option<u64>,
    pub(crate) category_name: Option<String>,
    pub(crate) channel_id: u64,
    pub(crate) channel_name: String,
    pub(crate) emoji_id: u64,
    pub(crate) emoji_name: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct ActivityLabels {
    pub(crate) guild_id: u64,
    pub(crate) activity_application_id: Option<u64>,
    pub(crate) activity_name: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MembersLabels {
    pub(crate) guild_id: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct MemberStatusLabels {
    pub(crate) guild_id: u64,
    pub(crate) status: String,
}

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

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct BoostLabels {
    pub(crate) guild_id: u64,
}

pub(crate) struct Handler {
    config: settings::Metrics,
    pub(crate) registry: Registry,
    pub(crate) guilds: Family<GuildsLabels, Gauge>,
    pub(crate) message_sent: Family<MessageSentLabels, Counter>,
    pub(crate) member: Family<MembersLabels, Gauge>,
    pub(crate) emote_sent: Family<EmoteSentLabels, Counter>,
    pub(crate) emote_reacted: Family<EmoteReactedLabels, Counter>,
    pub(crate) activity: Family<ActivityLabels, Gauge>,
    pub(crate) member_status: Family<MemberStatusLabels, Gauge>,
    pub(crate) member_voice: Family<MemberVoiceLabels, Gauge>,
    pub(crate) boost: Family<BoostLabels, Gauge>,
}

impl Handler {
    pub(crate) fn new(config: settings::Metrics) -> Self {
        let mut registry = <Registry>::with_prefix(&config.prefix);

        let guilds = Family::<GuildsLabels, Gauge>::default();
        registry.register(
            "guilds_total",
            "The total number of guilds.",
            guilds.clone(),
        );

        let message_sent = Family::<MessageSentLabels, Counter>::default();
        registry.register(
            "message_sent_total",
            "The total number of discord messages sent by users.",
            message_sent.clone(),
        );

        let emote_sent = Family::<EmoteSentLabels, Counter>::default();
        registry.register(
            "emote_sent_total",
            "The total number of discord emotes sent by users in messages.",
            emote_sent.clone(),
        );

        let emote_reacted = Family::<EmoteReactedLabels, Counter>::default();
        registry.register(
            "emote_reacted_total",
            "The total number of discord emotes reacted with by users in messages.",
            emote_reacted.clone(),
        );

        let activity = Family::<ActivityLabels, Gauge>::default();
        registry.register(
            "activity_total",
            "The total number of current activities.",
            activity.clone(),
        );

        let member = Family::<MembersLabels, Gauge>::default();
        registry.register(
            "member_total",
            "The total number of members on the guild.",
            member.clone(),
        );

        let member_status = Family::<MemberStatusLabels, Gauge>::default();
        registry.register(
            "member_status_total",
            "The total number of members on the guild per status.",
            member_status.clone(),
        );

        let member_voice = Family::<MemberVoiceLabels, Gauge>::default();
        registry.register(
            "member_voice_total",
            "The total number of members in voice channels.",
            member_voice.clone(),
        );

        let boost = Family::<BoostLabels, Gauge>::default();
        registry.register(
            "boost_total",
            "The total number of boosts on the guild.",
            boost.clone(),
        );

        Self {
            config,
            registry,
            // metrics
            guilds,
            message_sent,
            emote_sent,
            emote_reacted,
            activity,
            member,
            member_status,
            member_voice,
            boost,
        }
    }
}

#[instrument(skip(handler, shutdown))]
pub(crate) async fn serve(
    handler: Arc<Handler>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create webserver for metrics
    let rest_app = Router::new()
        .route("/metrics", get(metrics))
        .layer(Extension(Arc::clone(&handler)))
        .with_state(());

    // Bind tcp listener
    let listener = tokio::net::TcpListener::bind(&handler.config.address).await?;

    // Serve webserver and wait
    axum::serve(listener, rest_app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;

    Ok(())
}

async fn metrics(auth: Option<AuthBasic>, Extension(handler): Extension<Arc<Handler>>) -> Response {
    // Check basic auth
    if let Some(AuthBasic((username, password))) = auth {
        if username != handler.config.username || password != Some(handler.config.password.clone())
        {
            return (StatusCode::UNAUTHORIZED, "invalid auth").into_response();
        }
    } else {
        return (StatusCode::UNAUTHORIZED, "missing basic auth").into_response();
    }

    // Encode the metrics content into the buffer
    let mut buffer = String::new();
    encode(&mut buffer, &handler.registry).expect("failed to encode metrics into the buffer");

    // Respond with encoded metrics
    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )
        .body(Body::from(buffer))
        .expect("failed to build success target response")
}
