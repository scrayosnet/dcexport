//! This module implements the Discord guild listener. Any updates are applied to the metrics handler.

use crate::metrics;
use crate::metrics::{
    ActivityLabels, BoostLabels, BotLabels, ChannelLabels, EmoteUsedLabels, GuildsLabels,
    MemberLabels, MemberStatusLabels, MemberVoiceLabels, MessageSentLabels,
};
use serenity::all::{
    parse_emoji, ChannelId, Context, EventHandler, GatewayIntents, Guild, GuildChannel, GuildId,
    Member, Message, PartialGuild, Presence, Reaction, ReactionType, UnavailableGuild, User,
    UserId, VoiceState,
};
use serenity::{async_trait, Client};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::select;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

/// [`CachedUser`] is a bundle of information that should be cached. This cache is complementary to the
/// build-in serenity cache. It contains information required to decrement the prometheus gauges.
#[derive(Clone, Debug)]
pub struct CachedUser {
    presence: Presence,
}

/// [`Handler`] is the [servable](serve) Discord listener. It listens for Discord gateway events and
/// updates the [metrics](metrics::Handler) accordingly.
pub struct Handler {
    metrics_handler: Arc<metrics::Handler>,
    users: RwLock<HashMap<(GuildId, UserId), CachedUser>>,
}

impl Handler {
    /// Creates a new [`Handler`] for a [`metrics::Handler`]. Any updates are applied to these metrics.
    pub fn new(metrics_handler: Arc<metrics::Handler>) -> Self {
        Self {
            metrics_handler,
            users: RwLock::new(HashMap::new()),
        }
    }
}

/// Gets the root category and channel for a guild channel. It expects all relevant items to be cached.
fn category_channel(
    ctx: &Context,
    guild_id: GuildId,
    channel_id: ChannelId,
) -> (Option<ChannelId>, ChannelId) {
    // Get base
    let guild = ctx.cache.guild(guild_id).expect("Guild not found");
    let mut channel = &guild.channels[&channel_id];

    // Handle category
    let Some(parent_id) = channel.parent_id else {
        return (None, channel.id);
    };
    let category = &guild.channels[&parent_id];

    // Handle thread
    let Some(parent_id) = category.parent_id else {
        return (Some(category.id), channel.id);
    };
    channel = category;
    let category = &guild.channels[&parent_id];

    (Some(category.id), channel.id)
}

#[async_trait]
impl EventHandler for Handler {
    async fn guild_create(&self, ctx: Context, guild: Guild, _is_new: Option<bool>) {
        info!(guild_id = guild.id.get(), "Guild create");

        // Handle `guild` metric
        self.metrics_handler
            .guild
            .get_or_create(&GuildsLabels {
                guild_id: guild.id.into(),
                guild_name: guild.name.clone(),
            })
            .set(1);

        // Handle `channel` metric
        for channel in guild.channels.values() {
            self.metrics_handler
                .channel
                .get_or_create(&ChannelLabels {
                    guild_id: guild.id.get(),
                    channel_id: channel.id.get(),
                    channel_name: channel.name.clone(),
                    channel_nsfw: channel.nsfw.into(),
                    channel_type: channel.kind.name().to_string(),
                })
                .set(1);
        }

        // Handle `boost` metric
        self.metrics_handler
            .boost
            .get_or_create(&BoostLabels {
                guild_id: guild.id.into(),
            })
            .set(
                guild
                    .premium_subscription_count
                    .unwrap_or(0)
                    .try_into()
                    .expect("expected to fit in i64"),
            );

        // Handle `member` metric
        self.metrics_handler
            .member
            .get_or_create(&MemberLabels {
                guild_id: guild.id.into(),
            })
            .set(
                guild
                    .member_count
                    .try_into()
                    .expect("expected to fit in i64"),
            );

        // Handle `bot` metric
        let mut members_after = None;
        loop {
            let Ok(members) = guild.members(&ctx.http, None, members_after).await else {
                warn!(guild_id = guild.id.get(), "Failed to count guild bots");
                // Remove metric to indicate no bots were counted (successfully)
                self.metrics_handler.bot.remove(&BotLabels {
                    guild_id: guild.id.into(),
                });
                break;
            };
            self.metrics_handler
                .bot
                .get_or_create(&BotLabels {
                    guild_id: guild.id.into(),
                })
                .inc_by(
                    members
                        .iter()
                        .filter(|member| member.user.bot)
                        .count()
                        .try_into()
                        .expect("expected to fit in i64"),
                );
            let Some(last) = members.last() else {
                break;
            };
            members_after = Some(last.user.id);
        }

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

            // store user presences into handler cache such that the metrics can be decremented on the next presence update
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
                let (category, channel) = category_channel(&ctx, guild.id, *channel_id);
                self.metrics_handler
                    .member_voice
                    .get_or_create(&MemberVoiceLabels {
                        guild_id: guild.id.into(),
                        category_id: category.as_ref().map(|ch| ch.get()),
                        channel_id: channel.into(),
                        self_stream: voice.self_stream.unwrap_or(false).into(),
                        self_video: voice.self_video.into(),
                        self_deaf: voice.self_deaf.into(),
                        self_mute: voice.self_mute.into(),
                    })
                    .inc();
            }
        }
    }

    async fn guild_update(
        &self,
        _ctx: Context,
        old_data_if_available: Option<Guild>,
        new_data: PartialGuild,
    ) {
        info!(guild_id = new_data.id.get(), "Guild Update");

        // Handle `guild` metric
        if let Some(guild) = old_data_if_available {
            self.metrics_handler.guild.remove(&GuildsLabels {
                guild_id: guild.id.into(),
                guild_name: guild.name,
            });
        }

        // Handle `boost` metric
        self.metrics_handler
            .boost
            .get_or_create(&BoostLabels {
                guild_id: new_data.id.into(),
            })
            .set(
                new_data
                    .premium_subscription_count
                    .unwrap_or(0)
                    .try_into()
                    .expect("expected to fit in i64"),
            );
    }

    async fn guild_delete(&self, ctx: Context, incomplete: UnavailableGuild, full: Option<Guild>) {
        info!(guild_id = incomplete.id.get(), "Guild delete");

        let Some(guild) = full else {
            warn!(
                guild_id = incomplete.id.get(),
                "failed to get guild from cache, this might cause inconsistencies in the metrics"
            );
            return;
        };

        // Handle `guild` metric
        self.metrics_handler.guild.remove(&GuildsLabels {
            guild_id: incomplete.id.into(),
            guild_name: guild.name,
        });

        // Handle `channel` metric
        for channel in guild.channels.values() {
            self.metrics_handler.channel.remove(&ChannelLabels {
                guild_id: guild.id.get(),
                channel_id: channel.id.get(),
                channel_name: channel.name.clone(),
                channel_nsfw: channel.nsfw.into(),
                channel_type: channel.kind.name().to_string(),
            });
        }

        // Handle `boost` metric
        self.metrics_handler.boost.remove(&BoostLabels {
            guild_id: guild.id.into(),
        });

        // Handle `member` metric
        self.metrics_handler.member.remove(&MemberLabels {
            guild_id: guild.id.into(),
        });

        // Handle `bot` metric
        self.metrics_handler.bot.remove(&BotLabels {
            guild_id: incomplete.id.into(),
        });

        for (user_id, presence) in &guild.presences {
            // Handle `member_status` metric
            self.metrics_handler
                .member_status
                .remove(&MemberStatusLabels {
                    guild_id: guild.id.into(),
                    status: presence.status.name().to_string(),
                });

            // Handle `activity` metric
            for activity in &presence.activities {
                self.metrics_handler.activity.remove(&ActivityLabels {
                    guild_id: guild.id.into(),
                    activity_application_id: activity.application_id.map(Into::into),
                    activity_name: activity.name.clone(),
                });
            }

            // remove user presences from handler cache
            self.users.write().await.remove(&(guild.id, *user_id));
        }

        // Handle `member_voice` metric
        for (_, voice) in guild.voice_states {
            if let Some(channel_id) = &voice.channel_id {
                let (category, channel) = category_channel(&ctx, guild.id, *channel_id);
                self.metrics_handler
                    .member_voice
                    .remove(&MemberVoiceLabels {
                        guild_id: guild.id.into(),
                        category_id: category.as_ref().map(|ch| ch.get()),
                        channel_id: channel.into(),
                        self_stream: voice.self_stream.unwrap_or(false).into(),
                        self_video: voice.self_video.into(),
                        self_deaf: voice.self_deaf.into(),
                        self_mute: voice.self_mute.into(),
                    });
            }
        }
    }

    async fn channel_create(&self, _ctx: Context, channel: GuildChannel) {
        info!(
            guild_id = channel.guild_id.get(),
            channel_id = channel.id.get(),
            "Channel create"
        );

        self.metrics_handler
            .channel
            .get_or_create(&ChannelLabels {
                guild_id: channel.guild_id.get(),
                channel_id: channel.id.get(),
                channel_name: channel.name.clone(),
                channel_nsfw: channel.nsfw.into(),
                channel_type: channel.kind.name().to_string(),
            })
            .set(1);
    }

    async fn channel_update(&self, _ctx: Context, old: Option<GuildChannel>, new: GuildChannel) {
        info!(
            guild_id = new.guild_id.get(),
            channel_id = new.id.get(),
            "Channel update"
        );

        // Decrement old if available
        if let Some(old) = old {
            self.metrics_handler
                .channel
                .get_or_create(&ChannelLabels {
                    guild_id: old.guild_id.get(),
                    channel_id: old.id.get(),
                    channel_name: old.name.clone(),
                    channel_nsfw: old.nsfw.into(),
                    channel_type: old.kind.name().to_string(),
                })
                .set(0);
        }

        // Increment new
        self.metrics_handler
            .channel
            .get_or_create(&ChannelLabels {
                guild_id: new.guild_id.get(),
                channel_id: new.id.get(),
                channel_name: new.name.clone(),
                channel_nsfw: new.nsfw.into(),
                channel_type: new.kind.name().to_string(),
            })
            .set(1);
    }

    async fn channel_delete(
        &self,
        _ctx: Context,
        channel: GuildChannel,
        _messages: Option<Vec<Message>>,
    ) {
        info!(
            guild_id = channel.guild_id.get(),
            channel_id = channel.id.get(),
            "Channel delete"
        );

        self.metrics_handler.channel.remove(&ChannelLabels {
            guild_id: channel.guild_id.get(),
            channel_id: channel.id.get(),
            channel_name: channel.name.clone(),
            channel_nsfw: channel.nsfw.into(),
            channel_type: channel.kind.name().to_string(),
        });
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

        // Handle `bot` metric
        if new_member.user.bot {
            self.metrics_handler
                .bot
                .get_or_create(&BotLabels {
                    guild_id: new_member.guild_id.into(),
                })
                .inc();
        }
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
            .dec();

        // Handle `bot` metric
        if user.bot {
            self.metrics_handler
                .bot
                .get_or_create(&BotLabels {
                    guild_id: guild_id.into(),
                })
                .dec();
        }
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

        let (category, channel) = category_channel(&ctx, guild_id, msg.channel_id);

        // Handle `message_sent` metric
        self.metrics_handler
            .message_sent
            .get_or_create(&MessageSentLabels {
                guild_id: guild_id.into(),
                category_id: category.as_ref().map(|ch| ch.get()),
                channel_id: channel.into(),
            })
            .inc();

        // Handle `emote_used` metric
        for part in msg.content.split_whitespace() {
            let Some(emoji) = parse_emoji(part) else {
                // Only tracks custom emojis
                continue;
            };

            self.metrics_handler
                .emote_used
                .get_or_create(&EmoteUsedLabels {
                    guild_id: guild_id.into(),
                    category_id: category.as_ref().map(|ch| ch.get()),
                    channel_id: channel.into(),
                    reaction: false.into(),
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

        let (category, channel) = category_channel(&ctx, guild_id, add_reaction.channel_id);

        // Handle `emote_used` metric
        self.metrics_handler
            .emote_used
            .get_or_create(&EmoteUsedLabels {
                guild_id: guild_id.into(),
                category_id: category.as_ref().map(|ch| ch.get()),
                channel_id: channel.into(),
                reaction: true.into(),
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
        info!(
            guild_id = guild_id.get(),
            has_cached = old.is_some(),
            "Voice state update"
        );

        // Decrement gauges for previous state if cached
        'dec: {
            let Some(old) = old else {
                break 'dec;
            };

            // Get channel and category
            let Some(channel_id) = &old.channel_id else {
                // Also caused by user leaving to another guild
                warn!(
                    guild_id = guild_id.get(),
                    user_id = old.user_id.get(),
                    "failed to get old channel, this might cause inconsistencies in the metrics"
                );
                break 'dec;
            };

            let (category, channel) = category_channel(&ctx, guild_id, *channel_id);

            // Handle `member_voice` metric (decrement)
            self.metrics_handler
                .member_voice
                .get_or_create(&MemberVoiceLabels {
                    guild_id: guild_id.into(),
                    category_id: category.as_ref().map(|ch| ch.get()),
                    channel_id: channel.into(),
                    self_stream: old.self_stream.unwrap_or(false).into(),
                    self_video: old.self_video.into(),
                    self_deaf: old.self_deaf.into(),
                    self_mute: old.self_mute.into(),
                })
                .dec();
        }

        // Increment gauges for new state
        'inc: {
            // Get channel and category
            let Some(channel_id) = &new.channel_id else {
                // Also caused by user leaving to another guild
                warn!(
                    guild_id = guild_id.get(),
                    user_id = new.user_id.get(),
                    "failed to get new channel, this might cause inconsistencies in the metrics"
                );
                break 'inc;
            };

            let (category, channel) = category_channel(&ctx, guild_id, *channel_id);

            // Handle `member_voice` metric
            self.metrics_handler
                .member_voice
                .get_or_create(&MemberVoiceLabels {
                    guild_id: guild_id.into(),
                    category_id: category.as_ref().map(|ch| ch.get()),
                    channel_id: channel.into(),
                    self_stream: new.self_stream.unwrap_or(false).into(),
                    self_video: new.self_video.into(),
                    self_deaf: new.self_deaf.into(),
                    self_mute: new.self_mute.into(),
                })
                .inc();
        }
    }
}

/// Serves the [`Handler`] and starts listening for guild updates.
///
/// Use the [CancellationToken] to cancel and gracefully shutdown the [Handler].
#[instrument(skip(handler, shutdown))]
pub async fn serve(
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
        () = shutdown.cancelled() => {
            client.shard_manager.shutdown_all().await;
        }
    }

    Ok(())
}
