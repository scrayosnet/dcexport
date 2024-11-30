use crate::metrics;
use crate::metrics::{
    ActivityLabels, BoostLabels, EmoteReactedLabels, EmoteSentLabels, GuildsLabels, MemberLabels,
    MemberStatusLabels, MemberVoiceLabels, MessageSentLabels,
};
use axum::async_trait;
use serenity::all::{
    parse_emoji, ChannelId, Context, EventHandler, GatewayIntents, Guild, GuildChannel, GuildId,
    Member, Message, PartialGuild, Presence, Reaction, ReactionType, UnavailableGuild, User,
    UserId, VoiceState,
};
use serenity::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::select;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

/// [CachedUser] is a bundle of information that should be cached. This cache is complementary to the
/// build-in serenity cache. It contains information required to decrement the prometheus gauges.
#[derive(Clone, Debug)]
pub struct CachedUser {
    presence: Presence,
}

/// [Handler] is the [servable](serve) Discord listener. It listens for Discord gateway events and
/// updates the [metrics](metrics::Handler) accordingly.
pub struct Handler {
    metrics_handler: Arc<metrics::Handler>,
    users: RwLock<HashMap<(GuildId, UserId), CachedUser>>,
}

impl Handler {
    /// Creates a new [Handler] for a [metrics::Handler]. Any updates are applied to these metrics.
    pub fn new(metrics_handler: Arc<metrics::Handler>) -> Self {
        Self {
            metrics_handler,
            users: RwLock::new(HashMap::new()),
        }
    }

    /// Gets the root category and channel for a channel (any kind).
    async fn category_channel(
        &self,
        ctx: &Context,
        channel_id: &ChannelId,
    ) -> Result<(Option<GuildChannel>, GuildChannel), serenity::Error> {
        let mut channel = channel_id
            .to_channel(&ctx.http)
            .await?
            .guild()
            .expect("channel is not part of a guild");

        // Handle category
        let Some(parent_id) = channel.parent_id else {
            return Ok((None, channel));
        };
        let category = parent_id
            .to_channel(&ctx.http)
            .await?
            .guild()
            .expect("channel is not part of a guild");

        // Handle thread
        let Some(parent_id) = category.parent_id else {
            return Ok((Some(category), channel));
        };
        channel = category;
        let category = parent_id
            .to_channel(&ctx.http)
            .await?
            .guild()
            .expect("channel is not part of a guild");

        Ok((Some(category), channel))
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: Option<bool>) {
        info!(guild_id = guild.id.get(), "Guild create");

        // Handle `guilds` metric
        self.metrics_handler
            .guilds
            .get_or_create(&GuildsLabels {
                guild_id: guild.id.into(),
            })
            .set(1);

        // Handle `boost` metric
        self.metrics_handler
            .boost
            .get_or_create(&BoostLabels {
                guild_id: guild.id.into(),
            })
            .set(guild.premium_subscription_count.unwrap_or(0) as i64);

        // Handle `member` metric
        self.metrics_handler
            .member
            .get_or_create(&MemberLabels {
                guild_id: guild.id.into(),
            })
            .set(guild.member_count as i64);

        for (user_id, presence) in &guild.presences {
            debug!(user_id = user_id.get(), "create presence");

            // Handle `member_status` metric
            self.metrics_handler
                .member_status
                .get_or_create(&MemberStatusLabels {
                    guild_id: guild.id.into(),
                    status: presence.status.name().to_string(),
                })
                .inc();

            // Handle `activity` metric
            for activity in &presence.activities {
                self.metrics_handler
                    .activity
                    .get_or_create(&ActivityLabels {
                        guild_id: guild.id.into(),
                        activity_application_id: activity.application_id.map(Into::into),
                        activity_name: activity.name.clone(),
                    })
                    .inc();
            }

            // store user presences into handler cache such that the metrics can be decremented on
            // the next presence update
            self.users.write().await.insert(
                (guild.id, *user_id),
                CachedUser {
                    presence: presence.clone(),
                },
            );
        }

        // Handle `member_voice` metric
        for (_, voice) in guild.voice_states {
            if let Some(channel_id) = &voice.channel_id {
                let Ok((category, channel)) = self.category_channel(&ctx, channel_id).await else {
                    warn!(guild_id=guild.id.get(), channel_id=channel_id.get(), "failed to get category and channel, this might cause inconsistencies in the metrics!");
                    return;
                };
                self.metrics_handler
                    .member_voice
                    .get_or_create(&MemberVoiceLabels {
                        guild_id: guild.id.into(),
                        category_id: category.as_ref().map(|ch| ch.id.into()),
                        category_name: category.as_ref().map(|ch| ch.name.clone()),
                        channel_id: channel.id.into(),
                        channel_name: channel.name.clone(),
                        self_stream: voice.self_stream.unwrap_or(false).into(),
                        self_video: voice.self_video.into(),
                        self_deaf: voice.self_deaf.into(),
                        self_mute: voice.self_mute.into(),
                    })
                    .inc();
            }
        }
    }

    async fn guild_delete(
        &self,
        _ctx: Context,
        incomplete: UnavailableGuild,
        _full: Option<Guild>,
    ) {
        info!(guild_id = incomplete.id.get(), "Guild delete");

        // Handle `guilds` metric
        self.metrics_handler
            .guilds
            .get_or_create(&GuildsLabels {
                guild_id: incomplete.id.into(),
            })
            .set(0);
    }

    async fn guild_member_addition(&self, _ctx: Context, new_member: Member) {
        info!(
            guild_id = new_member.guild_id.get(),
            user_id = new_member.user.id.get(),
            "Guild member addition"
        );

        // Handle `member` metric
        self.metrics_handler
            .member
            .get_or_create(&MemberLabels {
                guild_id: new_member.guild_id.into(),
            })
            .inc();
    }

    async fn guild_member_removal(
        &self,
        _ctx: Context,
        guild_id: GuildId,
        user: User,
        _member_data_if_available: Option<Member>,
    ) {
        info!(
            guild_id = guild_id.get(),
            user_id = user.id.get(),
            "Guild member removal"
        );

        // Handle `member` metric
        self.metrics_handler
            .member
            .get_or_create(&MemberLabels {
                guild_id: guild_id.into(),
            })
            .inc();
    }

    async fn guild_update(
        &self,
        _ctx: Context,
        _old_data_if_available: Option<Guild>,
        new_data: PartialGuild,
    ) {
        info!(guild_id = new_data.id.get(), "Guild Update");

        self.metrics_handler
            .boost
            .get_or_create(&BoostLabels {
                guild_id: new_data.id.into(),
            })
            .set(new_data.premium_subscription_count.unwrap_or(0) as i64);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let Some(guild_id) = msg.guild_id else {
            // Only tracks guild events
            return;
        };
        info!(guild_id = guild_id.get(), "Message");

        if msg.author.bot || msg.author.system {
            // Only tracks user messages
            return;
        }

        let Ok((category, channel)) = self.category_channel(&ctx, &msg.channel_id).await else {
            warn!(guild_id=guild_id.get(), channel_id=msg.channel_id.get(), "failed to get category and channel, this might cause inconsistencies in the metrics!");
            return;
        };

        // Handle `message_sent` metric
        self.metrics_handler
            .message_sent
            .get_or_create(&MessageSentLabels {
                guild_id: guild_id.into(),
                category_id: category.as_ref().map(|ch| ch.id.into()),
                category_name: category.as_ref().map(|ch| ch.name.clone()),
                channel_id: channel.id.into(),
                channel_name: channel.name.clone(),
            })
            .inc();

        // Handle `emote_sent` metric
        for part in msg.content.split_whitespace() {
            let Some(emoji) = parse_emoji(part) else {
                // Only tracks custom emojis
                continue;
            };

            self.metrics_handler
                .emote_sent
                .get_or_create(&EmoteSentLabels {
                    guild_id: guild_id.into(),
                    category_id: category.as_ref().map(|ch| ch.id.into()),
                    category_name: category.as_ref().map(|ch| ch.name.clone()),
                    channel_id: channel.id.into(),
                    channel_name: channel.name.clone(),
                    emoji_id: emoji.id.into(),
                    emoji_name: Some(emoji.name),
                })
                .inc();
        }
    }

    async fn reaction_add(&self, ctx: Context, add_reaction: Reaction) {
        let Some(guild_id) = add_reaction.guild_id else {
            // Only tracks guild events
            return;
        };
        info!(guild_id = guild_id.get(), "Reaction add");

        if let Some(member) = &add_reaction.member {
            if member.user.bot || member.user.system {
                // Only tracks user messages
                return;
            }
        }

        let ReactionType::Custom { name, id, .. } = add_reaction.emoji else {
            // Only tracks custom emojis
            return;
        };

        let Ok((category, channel)) = self.category_channel(&ctx, &add_reaction.channel_id).await
        else {
            warn!(guild_id=guild_id.get(), channel_id=add_reaction.channel_id.get(), "failed to get category and channel, this might cause inconsistencies in the metrics!");
            return;
        };

        // Handle `emote_reacted` metric
        self.metrics_handler
            .emote_reacted
            .get_or_create(&EmoteReactedLabels {
                guild_id: guild_id.into(),
                category_id: category.as_ref().map(|ch| ch.id.into()),
                category_name: category.as_ref().map(|ch| ch.name.clone()),
                channel_id: channel.id.into(),
                channel_name: channel.name.clone(),
                emoji_id: id.into(),
                emoji_name: name,
            })
            .inc();
    }

    async fn presence_update(&self, _ctx: Context, new_data: Presence) {
        let Some(guild_id) = new_data.guild_id else {
            // Only tracks guild events
            return;
        };
        info!(
            guild_id = guild_id.get(),
            user_id = new_data.user.id.get(),
            "Presence update"
        );

        // Decrement gauges for previous state if cached
        if let Some(cached_user) = self.users.read().await.get(&(guild_id, new_data.user.id)) {
            // Handle `member_status` metric (decrement)
            self.metrics_handler
                .member_status
                .get_or_create(&MemberStatusLabels {
                    guild_id: guild_id.into(),
                    status: cached_user.presence.status.name().to_string(),
                })
                .dec();

            // Handle `activity` metric (decrement)
            for activity in &cached_user.presence.activities {
                self.metrics_handler
                    .activity
                    .get_or_create(&ActivityLabels {
                        guild_id: guild_id.into(),
                        activity_application_id: activity.application_id.map(Into::into),
                        activity_name: activity.name.clone(),
                    })
                    .dec();
            }
        }

        // Handle `member_status` metric
        self.metrics_handler
            .member_status
            .get_or_create(&MemberStatusLabels {
                guild_id: guild_id.into(),
                status: new_data.status.name().to_string(),
            })
            .inc();

        // Handle `activity` metric
        for activity in &new_data.activities {
            self.metrics_handler
                .activity
                .get_or_create(&ActivityLabels {
                    guild_id: guild_id.into(),
                    activity_application_id: activity.application_id.map(Into::into),
                    activity_name: activity.name.clone(),
                })
                .inc();
        }

        // Update cached state
        self.users.write().await.insert(
            (guild_id, new_data.user.id),
            CachedUser { presence: new_data },
        );
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        let Some(guild_id) = new.guild_id else {
            // Only tracks guild events
            return;
        };
        info!(guild_id = guild_id.get(), "Voice state update");

        // Decrement gauges for previous state if cached
        if let Some(old) = old {
            if let Some(channel_id) = &old.channel_id {
                let Ok((category, channel)) = self.category_channel(&ctx, channel_id).await else {
                    warn!(guild_id=guild_id.get(), channel_id=channel_id.get(), "failed to get category and channel, this might cause inconsistencies in the metrics!");
                    return;
                };

                // Handle `member_voice` metric (decrement)
                self.metrics_handler
                    .member_voice
                    .get_or_create(&MemberVoiceLabels {
                        guild_id: guild_id.into(),
                        category_id: category.as_ref().map(|ch| ch.id.into()),
                        category_name: category.as_ref().map(|ch| ch.name.clone()),
                        channel_id: channel.id.into(),
                        channel_name: channel.name.clone(),
                        self_stream: old.self_stream.unwrap_or(false).into(),
                        self_video: old.self_video.into(),
                        self_deaf: old.self_deaf.into(),
                        self_mute: old.self_mute.into(),
                    })
                    .dec();
            }
        }

        if let Some(channel_id) = &new.channel_id {
            let Ok((category, channel)) = self.category_channel(&ctx, channel_id).await else {
                warn!(guild_id=guild_id.get(), channel_id=channel_id.get(), "failed to get category and channel, this might cause inconsistencies in the metrics!");
                return;
            };

            // Handle `member_voice` metric
            self.metrics_handler
                .member_voice
                .get_or_create(&MemberVoiceLabels {
                    guild_id: guild_id.into(),
                    category_id: category.as_ref().map(|ch| ch.id.into()),
                    category_name: category.as_ref().map(|ch| ch.name.clone()),
                    channel_id: channel.id.into(),
                    channel_name: channel.name.clone(),
                    self_stream: new.self_stream.unwrap_or(false).into(),
                    self_video: new.self_video.into(),
                    self_deaf: new.self_deaf.into(),
                    self_mute: new.self_mute.into(),
                })
                .inc();
        }
    }
}

#[instrument(skip(handler, shutdown))]
pub(crate) async fn serve(
    discord_token: &str,
    handler: Handler,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::all();

    // Create a new instance of the Client, logging in as a bot
    let mut client = Client::builder(discord_token, intents)
        .event_handler(handler)
        .await?;

    select! {
        res = client.start_autosharded() => {
            if let Err(why) = res {
                return Err(why.into())
            }
        }
        _ = shutdown.cancelled() => {
            client.shard_manager.shutdown_all().await;
        }
    }

    Ok(())
}
