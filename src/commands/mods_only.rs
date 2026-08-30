use crate::{Context, Error, events::interaction_create::DiscordMemberRow};
use poise::{CreateReply, serenity_prelude as serenity};

/// Send message to logs channel
// Each argument is one optional embed field; collapsing them into a struct would
// change this function's signature and every call site, which this PR does not do.
#[allow(clippy::too_many_arguments)]
pub async fn log_embed(
    ctx: &serenity::Context,
    logs_channel_id: serenity::ChannelId,
    title: Option<String>,
    title_url: Option<String>,
    description: Option<String>,
    footer: Option<String>,
    colour: Option<String>,
    thumbnail_url: Option<String>,
    image_url: Option<String>,
    timestamp: Option<bool>,
) -> Result<(), Error> {
    let mut embed = serenity::CreateEmbed::new();

    // Set title and title URL
    if let Some(title) = title {
        embed = embed.title(title);
        if let Some(url) = title_url {
            embed = embed.url(url);
        }
    }

    // Set description
    if let Some(desc) = description {
        embed = embed.description(desc);
    }

    // Set footer
    if let Some(footer_text) = footer {
        embed = embed.footer(serenity::CreateEmbedFooter::new(footer_text));
    }

    // Set color (parse hex color)
    if let Some(color_str) = colour
        && let Ok(color_value) = u32::from_str_radix(color_str.trim_start_matches('#'), 16)
    {
        embed = embed.color(color_value);
    }

    // Set thumbnail
    if let Some(thumb_url) = thumbnail_url {
        embed = embed.thumbnail(thumb_url);
    }

    // Set image
    if let Some(img_url) = image_url {
        embed = embed.image(img_url);
    }

    // Set timestamp
    if timestamp.unwrap_or(false) {
        embed = embed.timestamp(serenity::Timestamp::now());
    }

    // Send the embed (logs_channel_id is parsed once at boot; see AppState).
    let builder = serenity::CreateMessage::new().embed(embed);

    // Do NOT .expect() here: this runs inside a message handler, and a panic on a
    // transient Discord failure takes the whole handler down for that message.
    if let Err(err) = logs_channel_id.send_message(ctx, builder).await {
        eprintln!("[log_embed] failed to write to logs channel: {err}");
    }

    Ok(())
}

/// Create a message embed
#[poise::command(
    track_edits,
    slash_command,
    required_permissions = "MANAGE_MESSAGES | MANAGE_THREADS"
)]
// These arguments are the slash command's options as Discord presents them;
// bundling them into a struct would change the command's public interface.
#[allow(clippy::too_many_arguments)]
pub async fn embed(
    ctx: Context<'_>,
    #[description = "Title of embed"] title: Option<String>,
    #[description = "URL for Title"] title_url: Option<String>,
    #[description = "Description for Embed"] description: Option<String>,
    #[description = "Footer text"] footer: Option<String>,
    #[description = "Embed colour"] colour: Option<String>,
    #[description = "Image URL for thumbnail"] thumbnail_url: Option<String>,
    #[description = "Image URL"] image_url: Option<String>,
    #[description = "Show timestamp"] timestamp: Option<bool>,
) -> Result<(), Error> {
    let mut embed = serenity::CreateEmbed::new();

    // Set title and title URL
    if let Some(title) = title {
        embed = embed.title(title);
        if let Some(url) = title_url {
            embed = embed.url(url);
        }
    }

    // Set description
    if let Some(desc) = description {
        embed = embed.description(desc);
    }

    // Set footer
    if let Some(footer_text) = footer {
        embed = embed.footer(serenity::CreateEmbedFooter::new(footer_text));
    }

    // Set color (parse hex color)
    if let Some(color_str) = colour
        && let Ok(color_value) = u32::from_str_radix(color_str.trim_start_matches('#'), 16)
    {
        embed = embed.color(color_value);
    }

    // Set thumbnail
    if let Some(thumb_url) = thumbnail_url {
        embed = embed.thumbnail(thumb_url);
    }

    // Set image
    if let Some(img_url) = image_url {
        embed = embed.image(img_url);
    }

    // Set timestamp
    if timestamp.unwrap_or(false) {
        embed = embed.timestamp(serenity::Timestamp::now());
    }

    // Send the embed
    ctx.send(CreateReply::default().embed(embed)).await?;

    Ok(())
}

/// Remove a member's verification link (COL-BOT-01).
///
/// This is the moderator-gated undo for a hijacked verification. It deletes the
/// `dsec_discord_members` row FIRST and only then strips the verified role, because
/// the reverse order can leave a member un-roled but still linked — the exact state
/// that permanently breaks `/member_info` for them. If the role removal fails the
/// row is already gone, so the reply and the log both flag that a human must strip
/// the role by hand. A member-facing `/unverify` is deliberately NOT provided: a
/// self-service unlink would let a hijacker cover their tracks.
#[poise::command(slash_command, required_permissions = "MANAGE_ROLES")]
pub async fn unlink(
    ctx: Context<'_>,
    #[description = "The member whose verification link should be removed"] user: serenity::User,
) -> Result<(), Error> {
    let state = &ctx.data().state;
    let user_id = user.id.to_string();

    // 1. Delete the link row FIRST. `.returning(...)` is required: without it
    //    PostgREST answers a DELETE with an empty 204 body that fails to deserialise
    //    (the COR-03 gotcha), and the returned rows also tell us whether a link
    //    actually existed. If this errors we stop before touching the role, so we
    //    never leave the member un-roled but still linked.
    let deleted: Vec<DiscordMemberRow> = state
        .supabase
        .database()
        .delete("dsec_discord_members")
        .eq("discord_id", &user_id)
        .returning("student_id,discord_id")
        .execute()
        .await?;
    let had_link = !deleted.is_empty();

    // 2. Then remove the verified role. Report clearly if THIS half fails so a human
    //    can finish it — the row is already gone, so nothing else is inconsistent.
    let role_id = state.verified_role_id;
    let mut role_removed = false;
    let mut role_error: Option<String> = None;
    match ctx.guild_id() {
        Some(guild_id) => match guild_id.member(ctx.serenity_context(), user.id).await {
            Ok(member) => match member.remove_role(ctx.serenity_context(), role_id).await {
                Ok(()) => role_removed = true,
                Err(err) => role_error = Some(err.to_string()),
            },
            Err(err) => role_error = Some(err.to_string()),
        },
        None => role_error = Some("command was not run in a guild".to_string()),
    }

    // Ephemeral report to the moderator.
    let mut summary = if had_link {
        String::from("Deleted the verification link row.\n")
    } else {
        String::from("No verification link row existed (nothing to delete).\n")
    };
    if role_removed {
        summary.push_str("Removed the verified role.");
    } else {
        summary.push_str(&format!(
            "⚠️ Could NOT remove the verified role — a human must remove <@&{role_id}> from <@{user_id}> by hand. Reason: {}.",
            role_error.as_deref().unwrap_or("unknown error")
        ));
    }

    ctx.send(
        CreateReply::default()
            .embed(serenity::CreateEmbed::new().title("Unlink").description(summary))
            .ephemeral(true),
    )
    .await?;

    // Log to the logs channel, naming the moderator who ran the command.
    let moderator = ctx.author();
    log_embed(
        ctx.serenity_context(),
        state.logs_channel_id,
        Some("Verification link removed (/unlink)".to_string()),
        None,
        Some(format!(
            "Moderator <@{}> (id `{}`) ran /unlink on <@{}> (id `{}`). Link row: {}. Verified role: {}.",
            moderator.id,
            moderator.id,
            user.id,
            user.id,
            if had_link { "deleted" } else { "none found" },
            if role_removed {
                "removed"
            } else {
                "NOT removed — needs manual follow-up"
            },
        )),
        None,
        None,
        None,
        None,
        Some(true),
    )
    .await?;

    Ok(())
}
