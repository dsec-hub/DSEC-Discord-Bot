use crate::{Data, Error, commands::mods_only::log_embed};
use poise::serenity_prelude as serenity;

/// Effective permissions for `member` in `channel_id`, honouring per-channel
/// permission overwrites. Returns `None` when the guild or the channel is not in
/// cache — the caller must treat that as "permissions could not be established"
/// and fail safe (do not ban), never as "no permissions".
fn channel_permissions(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    member: &serenity::Member,
) -> Option<serenity::Permissions> {
    let guild = ctx.cache.guild(guild_id)?;
    let channel = guild.channels.get(&channel_id)?;
    Some(guild.user_permissions_in(channel, member))
}

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

        // Never ban someone who can moderate, and fail SAFE: whenever the author's
        // identity or permissions cannot be established, abstain rather than ban.
        // A honeypot that occasionally misses a spammer is far cheaper than one
        // that bans a moderator or a legitimate member.
        let Some(message_guild_id) = new_message.guild_id else {
            eprintln!("[honeypot] message has no guild_id; abstaining from ban");
            return Ok(());
        };
        let member = match message_guild_id.member(ctx, user.id).await {
            Ok(member) => member,
            Err(err) => {
                eprintln!(
                    "[honeypot] could not fetch member {} to check permissions; abstaining: {err}",
                    user.id
                );
                return Ok(());
            }
        };
        // Effective permissions IN THE HONEYPOT CHANNEL (honouring overwrites).
        let Some(perms) = channel_permissions(ctx, message_guild_id, *current_channel_id, &member)
        else {
            eprintln!(
                "[honeypot] could not compute permissions for {} in {}; abstaining",
                user.id, current_channel_id
            );
            return Ok(());
        };
        if perms.ban_members() || perms.manage_messages() || perms.administrator() {
            return Ok(());
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

/// Discord rejects a thread name that is empty or over 100 characters, and the
/// resulting 400 is invisible to the person who posted (no logging is set up —
/// see OPS-04). So clamp here rather than letting the API reject it.
const MAX_THREAD_NAME_CHARS: usize = 100;
const DEFAULT_THREAD_NAME: &str = "LeetCode discussion";

/// Derive a thread name from a LeetCode post.
///
/// Posts are conventionally numbered ("1. Two Sum"), so the leading two
/// characters are dropped. A first line of two characters or fewer leaves
/// nothing behind, and a very long one exceeds Discord's limit — both are
/// handled here rather than at the API.
fn create_title_from_message(message: &str) -> String {
    let first = message.lines().next().unwrap_or("");
    let start = first
        .char_indices()
        .nth(2)
        .map(|(i, _)| i)
        .unwrap_or(first.len());
    let trimmed = first[start..].trim();

    if trimmed.is_empty() {
        return DEFAULT_THREAD_NAME.to_string();
    }
    // Truncate on a CHARACTER boundary — byte slicing would panic mid-emoji.
    trimmed.chars().take(MAX_THREAD_NAME_CHARS).collect()
}

async fn create_leetcode_thread(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    let leetcode_channel_id = data.state.leetcode_channel_id;

    let current_channel_id = &new_message.channel_id;

    if leetcode_channel_id.eq(current_channel_id) {
        let new_thread =
            serenity::CreateThread::new(create_title_from_message(&new_message.content));
        if let Err(err) = current_channel_id
            .create_thread_from_message(ctx, new_message.id, new_thread)
            .await
        {
            eprintln!(
                "[leetcode] failed to create thread for message {}: {err}",
                new_message.id
            );
        }
    }

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

#[cfg(test)]
mod tests {
    use super::create_title_from_message;

    #[test]
    fn strips_the_numbering_prefix() {
        assert_eq!(create_title_from_message("1. Two Sum"), "Two Sum");
    }

    #[test]
    fn empty_message_falls_back_to_the_default() {
        assert_eq!(create_title_from_message(""), "LeetCode discussion");
    }

    #[test]
    fn two_character_line_falls_back_to_the_default() {
        // "hi" has nothing left after the two-character prefix is dropped,
        // and Discord rejects an empty thread name with a 400.
        assert_eq!(create_title_from_message("hi"), "LeetCode discussion");
    }

    #[test]
    fn long_line_is_clamped_to_the_discord_limit() {
        let long = format!("1. {}", "a".repeat(200));
        let got = create_title_from_message(&long);
        assert_eq!(got.chars().count(), 100);
    }

    #[test]
    fn multibyte_prefix_does_not_panic_and_truncates_on_a_char_boundary() {
        // A leading emoji is two chars wide in some fonts but one char here;
        // byte-slicing this would panic.
        let got = create_title_from_message("🔥 Daily challenge: reverse a linked list");
        assert!(!got.is_empty());
    }

    #[test]
    fn only_the_first_line_is_used() {
        assert_eq!(
            create_title_from_message("1. Two Sum\nsome body text"),
            "Two Sum"
        );
    }
}
