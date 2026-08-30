use std::time::Duration;

use crate::{
    Data, Error,
    commands::verification::{StudentRow, VerificationModal},
};
use ::serenity::{
    all::{
        ComponentInteraction, Context, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
        CreateInteractionResponseMessage, GuildId, ModalInteraction, RoleId,
        collector::ModalInteractionCollector,
    },
    model::guild::Member,
};
use poise::Modal as _;
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct DiscordMemberRow {
    pub student_id: String,
    pub discord_id: String,
}

/// Wrap an embed into an ephemeral interaction response.
fn ephemeral_embed(embed: CreateEmbed) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .add_embed(embed)
            .ephemeral(true),
    )
}

/// Show the verification modal and wait for (and parse) the submission.
///
/// Returns `None` when the user let the modal time out or the submission
/// could not be parsed (an error response is sent in the latter case).
async fn collect_verification_modal(
    ctx: &Context,
    component_interaction: &ComponentInteraction,
) -> Result<Option<(ModalInteraction, VerificationModal)>, Error> {
    let modal_custom_id = component_interaction.id.to_string();

    let modal = VerificationModal::create(None, modal_custom_id.clone());
    component_interaction.create_response(ctx, modal).await?;

    let modal_submit = ModalInteractionCollector::new(&ctx.shard)
        .filter(move |d| d.data.custom_id == modal_custom_id)
        .timeout(Duration::from_secs(120))
        .await;

    let Some(modal_submit) = modal_submit else {
        return Ok(None);
    };

    match VerificationModal::parse(modal_submit.data.clone()) {
        Ok(data) => Ok(Some((modal_submit, data))),
        Err(_) => {
            modal_submit
                .create_response(
                    ctx,
                    ephemeral_embed(
                        CreateEmbed::new()
                            .title("Error")
                            .description("Failed to parse modal data. Please try again."),
                    ),
                )
                .await?;
            Ok(None)
        }
    }
}

/// Add to dsec_discord_members table (skips insert if already recorded).
async fn add_dsec_discord_table(
    data: &Data,
    student_id: &str,
    member_id: &String,
) -> Result<(), Error> {
    if member_recorded(data, member_id).await? {
        return Ok(());
    }

    let new_member = serde_json::json!({
        "student_id": student_id,
        "discord_id": member_id,
    });

    // `.returning(...)` makes supabase-lib-rs send `Prefer: return=representation`.
    // Without it PostgREST defaults a POST to `return=minimal` — a 201 with an
    // empty body — and deserialising that empty body into Vec<DiscordMemberRow>
    // failed, aborting before the role grant even though the row was written.
    // That is why verification failed on every member's first attempt (COR-03).
    let _: Vec<DiscordMemberRow> = data
        .state
        .supabase
        .database()
        .insert("dsec_discord_members")
        .values(new_member)?
        .returning("student_id,discord_id")
        .execute()
        .await?;

    Ok(())
}

/// Assign the verified role and send the success response.
async fn grant_verified_role(
    ctx: &Context,
    data: &Data,
    modal_submit: &ModalInteraction,
    discord_member: &Member,
    student_id: &str,
    verified_role_id: RoleId,
    via_cache: bool,
) -> Result<(), Error> {
    add_dsec_discord_table(data, student_id, &discord_member.user.id.to_string()).await?;
    discord_member.add_role(ctx, verified_role_id).await?;

    let mut embed = CreateEmbed::new().title("Verified ✅").description(format!(
        "You have been assigned the <@&{}> role!",
        verified_role_id
    ));
    if via_cache {
        embed = embed.footer(CreateEmbedFooter::new("⚡ via cache"));
    }

    modal_submit
        .create_response(ctx, ephemeral_embed(embed))
        .await?;
    Ok(())
}

/// Lower-case, trim, and collapse runs of internal whitespace to one space.
/// Used for every name comparison so a stray space or a double space in the
/// DUSA roster never rejects a real member.
fn normalise_name(raw: &str) -> String {
    raw.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Trim, lower-case, strip a leading "s", and drop spaces so a pasted
/// "s123 456 789 " looks up as "123456789".
fn normalise_student_id(raw: &str) -> String {
    let lowered: String = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    lowered.strip_prefix('s').unwrap_or(&lowered).to_string()
}

/// Whether the submitted name matches the roster name closely enough to be the
/// same person.
///
/// The first and last name tokens must BOTH match, and any tokens the student
/// typed in between must appear in the roster name in order — so an omitted
/// middle name is fine, but a single token, an arbitrary subset, a reordered
/// name, or a wrong surname is not. This is deliberately strict: verification is
/// already weak identity evidence (a name plus a student id), and a looser rule
/// would let a student id plus one common name token ("John", "Doe") claim the
/// verified role for someone else.
fn name_matches(roster: &str, submitted: &str) -> bool {
    let roster = normalise_name(roster);
    let submitted = normalise_name(submitted);

    let roster_words: Vec<&str> = roster.split_whitespace().collect();
    let submitted_words: Vec<&str> = submitted.split_whitespace().collect();

    // A single token (or empty) is far too weak to identify a person, and a roster
    // row without a distinct first and last name cannot be matched safely.
    if submitted_words.len() < 2 || roster_words.len() < 2 {
        return false;
    }

    // The first and last name must both match.
    if submitted_words.first() != roster_words.first()
        || submitted_words.last() != roster_words.last()
    {
        return false;
    }

    // Every token the student typed must appear in the roster name in order.
    let mut idx = 0usize;
    for &word in &submitted_words {
        match roster_words[idx..].iter().position(|&w| w == word) {
            Some(offset) => idx += offset + 1,
            None => return false,
        }
    }
    true
}

/// Whether the cached name for `student_id` matches the submitted `name`.
fn cached_name_matches(data: &Data, student_id: &str, name: &str) -> bool {
    let cache = data
        .state
        .student_cache
        .lock()
        .expect("Failed to get cache");
    match cache.get(student_id) {
        Some(cached_name) => name_matches(cached_name, name),
        None => false,
    }
}

/// Store the resolved student name in the cache (normalised for comparison).
fn cache_student(data: &Data, student_id: &str, full_name: &str) {
    let mut cache = data.state.student_cache.lock().unwrap();
    cache.insert(student_id.to_string(), normalise_name(full_name));
}

/// Look up a student by id in the database.
async fn fetch_student(data: &Data, student_id: &str) -> Result<Option<StudentRow>, Error> {
    let student_data: Vec<StudentRow> = data
        .state
        .supabase
        .database()
        .from("active_members")
        .select("full_name, student_id")
        .eq("student_id", student_id)
        .eq("membership_status", "Active")
        .execute()
        .await?;
    Ok(student_data.into_iter().next())
}

async fn member_recorded(data: &Data, user_id: &str) -> Result<bool, Error> {
    let rows: Vec<serde_json::Value> = data
        .state
        .supabase
        .database()
        .from("dsec_discord_members")
        .select("discord_id")
        .eq("discord_id", user_id)
        .execute()
        .await?;
    Ok(!rows.is_empty())
}

/// Handle a click on the "verify" button: collect the modal, then verify the
/// submitted student id/name against the cache and database.
async fn handle_verify(
    ctx: &Context,
    component_interaction: &ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    let Some(guild_id) = component_interaction.guild_id else {
        component_interaction
            .create_response(
                ctx,
                ephemeral_embed(
                    CreateEmbed::new()
                        .title("Unable to perform action")
                        .description("Action can only be performed in the DSEC server"),
                ),
            )
            .await?;
        return Ok(());
    };

    let verified_role_id = data.state.verified_role_id;

    // Fast, no-network "already verified" check using the member data that is
    // already attached to the button interaction. Anything slower than this
    // (a DB query, a member fetch) must NOT run before the modal is shown, or
    // Discord's ~3s acknowledgement window elapses and the click fails.
    if let Some(member) = &component_interaction.member
        && member.roles.contains(&verified_role_id)
    {
        component_interaction
            .create_response(
                ctx,
                ephemeral_embed(CreateEmbed::new().title("Already Verified ✅").description(
                    format!("You already have the <@&{}> role!", verified_role_id),
                )),
            )
            .await?;
        return Ok(());
    }

    // Respond to the click with the modal immediately.
    let Some((modal_submit, modal_data)) =
        collect_verification_modal(ctx, component_interaction).await?
    else {
        return Ok(());
    };

    // From here on we hold the modal-submit token, so the slower member fetch and
    // database work below is no longer racing the button's ack window. Wrap that
    // work so any failure (e.g. a database error) still sends the user an
    // ephemeral message rather than leaving a dead "This interaction failed"
    // interaction — poise's on_error cannot reach this modal submission (COR-03).
    let user_id = component_interaction.user.id;

    let verify_result: Result<(), Error> = async {
        let discord_member = GuildId::member(guild_id, ctx, user_id).await?;

        let student_id = normalise_student_id(&modal_data.student_id);

        if cached_name_matches(data, &student_id, &modal_data.name) {
            grant_verified_role(
                ctx,
                data,
                &modal_submit,
                &discord_member,
                &student_id,
                verified_role_id,
                true,
            )
            .await?;
            return Ok(());
        }

        let Some(student) = fetch_student(data, &student_id).await? else {
            modal_submit
                .create_response(
                    ctx,
                    ephemeral_embed(
                        CreateEmbed::new().title("Student ID not found!").description(
                            "Your student ID is not found.\nIt takes up to **a week** for your membership to be updated in the database since sign up.\nTry again later.",
                        ),
                    ),
                )
                .await?;
            return Ok(());
        };

        cache_student(data, &student_id, &student.full_name);

        if name_matches(&student.full_name, &modal_data.name) {
            grant_verified_role(
                ctx,
                data,
                &modal_submit,
                &discord_member,
                &student_id,
                verified_role_id,
                false,
            )
            .await?;
        } else {
            modal_submit
                .create_response(
                    ctx,
                    ephemeral_embed(CreateEmbed::new().title("Name mismatch ❌").description(
                        "Your student ID is present, however the name does not match. Try again.",
                    )),
                )
                .await?;
        }

        Ok(())
    }
    .await;

    if let Err(err) = verify_result {
        eprintln!("[verify] verification failed after modal submit: {err}");
        // Best-effort ephemeral error so the user does not see the generic
        // "This interaction failed" with no way forward.
        let _ = modal_submit
            .create_response(
                ctx,
                ephemeral_embed(
                    CreateEmbed::new()
                        .title("Something went wrong")
                        .description(
                            "A maintainer has been notified. Please try again in a minute.",
                        ),
                ),
            )
            .await;
    }

    Ok(())
}

pub async fn on_interaction_create(
    ctx: &Context,
    interaction: &serenity::Interaction,
    data: &Data,
) -> Result<(), Error> {
    let Some(component_interaction) = interaction.as_message_component() else {
        return Ok(());
    };

    if component_interaction.data.custom_id == "verify" {
        handle_verify(ctx, component_interaction, data).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_names() {
        assert!(name_matches("John Doe", "  john   doe  "));
        assert!(name_matches("John Doe", "JOHN DOE"));
    }

    #[test]
    fn verifies_full_name_and_omitted_middle() {
        assert!(name_matches("John Michael Doe", "John Michael Doe"));
        assert!(name_matches("John Michael Doe", "John Doe"));
        assert!(name_matches("John Michael Doe", "john michael doe"));
    }

    #[test]
    fn rejects_a_single_token() {
        // A student id plus one common name token must never verify.
        assert!(!name_matches("John Michael Doe", "John"));
        assert!(!name_matches("John Michael Doe", "Doe"));
        assert!(!name_matches("John Michael Doe", "Michael"));
        assert!(!name_matches("John Doe", "John"));
    }

    #[test]
    fn rejects_wrong_surname_or_first_name() {
        assert!(!name_matches("John Michael Doe", "John Smith"));
        assert!(!name_matches("John Michael Doe", "Jane Doe"));
        assert!(!name_matches("John Doe", "Jack Doe"));
    }

    #[test]
    fn rejects_a_different_person() {
        assert!(!name_matches("John Michael Doe", "Jane Doe"));
        assert!(!name_matches("John Doe", "Doe John"));
        assert!(!name_matches("John Doe", ""));
    }

    #[test]
    fn normalises_student_ids() {
        assert_eq!(normalise_student_id("s123456789 "), "123456789");
        assert_eq!(normalise_student_id("S123456789"), "123456789");
        assert_eq!(normalise_student_id(" 123 456 789 "), "123456789");
    }
}
