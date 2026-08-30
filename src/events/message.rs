use crate::{Data, Error, commands::mods_only::log_embed};
use poise::serenity_prelude as serenity;

async fn honeypot(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    // Config is parsed once at boot and stored on AppState (COL-BOT-05).
    let honeypot_channel_id = data.state.honeypot_channel_id;
    let current_channel_id = &new_message.channel_id;

    if honeypot_channel_id.eq(current_channel_id) {
        let user = &new_message.author;

        // Never ban a bot, a webhook, or ourselves. The honeypot exists to catch
        // spam accounts; banning another club integration (or the bot itself)
        // because it posted in the wrong channel is a self-inflicted outage.
        if user.bot || new_message.webhook_id.is_some() || user.id == ctx.cache.current_user().id {
            return Ok(());
        }

        // Never ban someone who can moderate. A moderator checking whether the
        // honeypot works should not be the person it catches. This reads the
        // cached guild; if it is unavailable we fall through and let the honeypot
        // act, rather than skipping the check silently for everyone.
        if let Some(guild_id) = new_message.guild_id
            && let Ok(member) = guild_id.member(ctx, user.id).await
        {
            // Guild-level moderator identity is exactly what we want here; the
            // deprecation is about per-channel permission overwrites, which are
            // irrelevant to "can this person moderate at all".
            #[allow(deprecated)]
            let perms = member.permissions(ctx);
            if let Ok(perms) = perms
                && (perms.ban_members() || perms.manage_messages() || perms.administrator())
            {
                return Ok(());
            }
        }

        let username = &user.name;
        let user_id = &user.id;
        let avatar_url = user.avatar_url();
        let ban_user = data
            .state
            .guild_id
            .ban_with_reason(ctx, user_id, 2, "Message sent in honeypot channel.")
            .await;

        match ban_user {
            Ok(()) => {
                log_embed(
                    ctx,
                    data.state.logs_channel_id,
                    Some("Honeypot activated!".to_string()),
                    None,
                    Some(format!("User got banned: {}", username)),
                    Some(format!("ID: {}", user_id)),
                    None,
                    avatar_url,
                    None,
                    Some(true),
                )
                .await?;
            }
            // A honeypot that cannot ban is information the mods need.
            Err(err) => {
                eprintln!(
                    "[honeypot] failed to ban {username} ({user_id}) in the honeypot channel: {err}"
                );
            }
        }
    }

    Ok(())
}

async fn create_leetcode_thread(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    let leetcode_channel_id = data.state.leetcode_channel_id;

    let current_channel_id = &new_message.channel_id;

    if leetcode_channel_id.eq(current_channel_id) {
        fn create_title_from_message(message: impl Into<String>) -> String {
            let message_string: String = message.into();

            message_string
                .lines()
                .next()
                .map(|line| {
                    let start = line
                        .char_indices()
                        .nth(2)
                        .map(|(i, _)| i)
                        .unwrap_or(line.len());
                    &line[start..]
                })
                .unwrap_or("")
                .to_string()
        }

        let new_thread =
            serenity::CreateThread::new(create_title_from_message(&new_message.content));
        current_channel_id
            .create_thread_from_message(ctx, new_message.id, new_thread)
            .await?;
    };

    Ok(())
}

pub async fn on_message(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    // Independent features. A failure in one must not skip the other, and neither
    // should propagate out of the event handler, where the default on_error only
    // eprintln!s (OPS-04).
    if let Err(err) = honeypot(ctx, new_message, data).await {
        eprintln!("[on_message] honeypot failed: {err}");
    }
    if let Err(err) = create_leetcode_thread(ctx, new_message, data).await {
        eprintln!("[on_message] leetcode thread failed: {err}");
    }
    Ok(())
}
