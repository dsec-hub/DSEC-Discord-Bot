use crate::{
    AppState, ApplicationContext, Context, Error,
    events::interaction_create::{DiscordMemberRow, user_attempt_lock},
    redact_digits,
};
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

/// Whether a `dsec_discord_members` row exists for `discord_id`. Used as a read-back
/// to disambiguate a delete whose HTTP call errored (SEC-19 new #A).
async fn link_row_exists(state: &AppState, discord_id: &str) -> Result<bool, Error> {
    let rows: Vec<serde_json::Value> = state
        .supabase
        .database()
        .from("dsec_discord_members")
        .select("discord_id")
        .eq("discord_id", discord_id)
        .execute()
        .await?;
    Ok(!rows.is_empty())
}

/// Build an ephemeral edit for the deferred `/unlink` response.
fn unlink_reply(title: &str, description: impl Into<String>) -> serenity::EditInteractionResponse {
    serenity::EditInteractionResponse::new().embed(
        serenity::CreateEmbed::new()
            .title(title)
            .description(description),
    )
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
///
/// Uses `ApplicationContext` so it can `defer_response` and then EDIT that one deferred
/// ephemeral (poise's `ctx.send` after a defer posts a *followup* in this version,
/// leaving the "thinking…" placeholder dangling — SEC-19 #7b).
///
/// AuthZ note: the `MANAGE_ROLES` gate is for command visibility only and is NOT
/// trusted for authorization — poise 0.6 treats a DM invoker as `Permissions::all()`,
/// so the gate passes in a DM. The command is `guild_only` and, before any database
/// work, asserts it is running in the DSEC guild specifically, which also stops a
/// moderator of some *other* guild the bot is in from deleting DSEC rows (SEC-19 #1).
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_ROLES")]
pub async fn unlink(
    ctx: ApplicationContext<'_>,
    #[description = "The member whose verification link should be removed"] user: serenity::User,
) -> Result<(), Error> {
    let state = &ctx.data.state;
    let serenity_ctx = ctx.serenity_context;
    let interaction = ctx.interaction;

    // AuthZ, before anything else and before any DB op: must be the DSEC guild.
    if interaction.guild_id != Some(state.guild_id) {
        interaction
            .create_response(
                serenity_ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(
                            serenity::CreateEmbed::new()
                                .title("Unavailable here")
                                .description("This command can only be used in the DSEC server."),
                        )
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    // Acknowledge within Discord's ~3s window before any DB/HTTP work. If the defer
    // itself fails the interaction is dead — ABORT before any mutation (SEC-19 #7a).
    if let Err(err) = ctx.defer_response(true).await {
        eprintln!("[unlink] defer failed: {}", redact_digits(&err.to_string()));
        return Ok(());
    }

    // Serialize against the TARGET user's own verification (SEC-19): this holds the
    // same per-user async lock verification uses, keyed on the user being unlinked, so
    // an in-flight verify for them cannot interleave with our delete + role removal and
    // leave a role-with-no-row (or the inverse). Acquired AFTER defer so waiting on a
    // running verify never eats the ack window; the tokio guard is held across the DB
    // and Discord calls below.
    let target_lock = user_attempt_lock(ctx.data, user.id);
    let _target_guard = target_lock.lock().await;

    let user_id = user.id.to_string();
    let moderator = &interaction.user;

    // 1. Delete the link row FIRST. `.returning(...)` is required (COR-03 empty-204).
    //    On a delete error we must NOT assert "nothing changed": the DELETE may have
    //    committed before a response/body failure. Read the row back to disambiguate,
    //    and report UNKNOWN only if the read-back also fails (SEC-19 #A). Errors are
    //    redacted before printing and never shown to the user (SEC-19 #9).
    let link_status: &str = match state
        .supabase
        .database()
        .delete("dsec_discord_members")
        .eq("discord_id", &user_id)
        .returning("student_id,discord_id")
        .execute::<DiscordMemberRow>()
        .await
    {
        Ok(rows) if rows.is_empty() => "no link row existed",
        Ok(_) => "deleted",
        Err(err) => {
            eprintln!(
                "[unlink] delete failed for target {}: {}",
                user.id,
                redact_digits(&err.to_string())
            );
            match link_row_exists(state, &user_id).await {
                // Row still present: the delete genuinely did not happen.
                Ok(true) => {
                    let _ = interaction
                        .edit_response(
                            serenity_ctx,
                            unlink_reply(
                                "Unlink failed",
                                "The database returned an error and the link row is still present (confirmed by read-back). Nothing changed. Try again, or remove it by hand — see SECURITY.md.",
                            ),
                        )
                        .await;
                    let _ = log_embed(
                        serenity_ctx,
                        state.logs_channel_id,
                        Some("Unlink FAILED (no change)".to_string()),
                        None,
                        Some(format!(
                            "Moderator <@{}> (id `{}`) ran /unlink on <@{}> (id `{}`): the delete errored and a read-back confirms the row is still present. No changes made.",
                            moderator.id, moderator.id, user.id, user.id
                        )),
                        None,
                        None,
                        None,
                        None,
                        Some(true),
                    )
                    .await;
                    return Ok(());
                }
                // Row gone: the delete actually committed; fall through to role removal.
                Ok(false) => "deleted (confirmed by read-back after a write error)",
                // Read-back also failed: genuinely UNKNOWN — do not touch the role.
                Err(err2) => {
                    eprintln!(
                        "[unlink] read-back after delete error failed for target {}: {}",
                        user.id,
                        redact_digits(&err2.to_string())
                    );
                    let _ = interaction
                        .edit_response(
                            serenity_ctx,
                            unlink_reply(
                                "Unlink status UNKNOWN",
                                "A database error occurred and a follow-up read could not confirm whether the link row was removed. It MAY already be gone. Verify by hand before relying on this — see SECURITY.md. The role was not touched.",
                            ),
                        )
                        .await;
                    let _ = log_embed(
                        serenity_ctx,
                        state.logs_channel_id,
                        Some("Unlink UNKNOWN (database error)".to_string()),
                        None,
                        Some(format!(
                            "Moderator <@{}> (id `{}`) ran /unlink on <@{}> (id `{}`): the delete errored and a read-back also failed. The link row may or may not be removed; manual verification required. Role not touched.",
                            moderator.id, moderator.id, user.id, user.id
                        )),
                        None,
                        None,
                        None,
                        None,
                        Some(true),
                    )
                    .await;
                    return Ok(());
                }
            }
        }
    };

    // 2. Then remove the verified role. We reach here only when the row is gone or
    //    never existed. Report clearly if THIS half fails so a human can finish it.
    //    Role errors are Discord API errors (no student id), safe to show the mod.
    let role_id = state.verified_role_id;
    let mut role_removed = false;
    let mut role_error: Option<String> = None;
    match state.guild_id.member(serenity_ctx, user.id).await {
        Ok(member) => match member.remove_role(serenity_ctx, role_id).await {
            Ok(()) => role_removed = true,
            Err(err) => role_error = Some(err.to_string()),
        },
        Err(err) => role_error = Some(err.to_string()),
    }

    // Report to the moderator by EDITING the deferred response (exactly one ephemeral,
    // not a followup — SEC-19 #7b). Best-effort so a failed reply cannot skip the audit.
    let mut summary = format!("Link row: {link_status}.\n");
    if role_removed {
        summary.push_str("Removed the verified role.");
    } else {
        summary.push_str(&format!(
            "⚠️ Could NOT remove the verified role — a human must remove <@&{role_id}> from <@{user_id}> by hand. Reason: {}.",
            role_error.as_deref().unwrap_or("unknown error")
        ));
    }
    let _ = interaction
        .edit_response(serenity_ctx, unlink_reply("Unlink", summary))
        .await;

    // Audit to the logs channel, naming the moderator — runs regardless of the reply.
    let _ = log_embed(
        serenity_ctx,
        state.logs_channel_id,
        Some("Verification link removed (/unlink)".to_string()),
        None,
        Some(format!(
            "Moderator <@{}> (id `{}`) ran /unlink on <@{}> (id `{}`). Link row: {link_status}. Verified role: {}.",
            moderator.id,
            moderator.id,
            user.id,
            user.id,
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
    .await;

    Ok(())
}
