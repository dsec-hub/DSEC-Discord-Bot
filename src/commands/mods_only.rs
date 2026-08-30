use crate::{Context, Error};
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
