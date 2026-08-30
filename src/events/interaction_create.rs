use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{
    Data, Error,
    commands::{
        mods_only::log_embed,
        verification::{StudentRow, VerificationModal},
    },
};
use ::serenity::{
    all::{
        ComponentInteraction, Context, CreateEmbed, CreateInteractionResponse,
        CreateInteractionResponseMessage, GuildId, ModalInteraction, RoleId, UserId,
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

/// The single verification-failure embed (SEC-19).
///
/// Every failure path — student id not on the roster, name mismatch, rate-limit
/// refusal, and a student id already claimed by a different Discord account —
/// renders this exact embed. Because there is one constructor the responses cannot
/// drift apart, so the flow can no longer be used as an oracle for whether a given
/// student id holds an Active DSEC membership.
fn verification_failed_embed() -> CreateEmbed {
    CreateEmbed::new().title("Verification failed").description(
        "We couldn't verify that name and student ID. Check both and try again — it can take up to a week after signing up for your membership to appear.",
    )
}

/// After this many failed attempts inside `ATTEMPT_WINDOW`, a user is refused with
/// no database round trip. Kept deliberately generous — a real member fixing a typo
/// in a hyphenated name must not be locked out — and the counter only moves on a
/// genuine failure (SEC-19).
const MAX_FAILURES: u32 = 5;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Per-Discord-user failed-verification counter: `(failures, window_start)`.
type AttemptMap = Mutex<HashMap<UserId, (u32, Instant)>>;

/// Whether `user_id` has already failed `MAX_FAILURES` times within the current
/// window. A window that has fully elapsed is treated as no failures.
fn is_rate_limited(attempts: &AttemptMap, user_id: UserId) -> bool {
    let attempts = attempts.lock().expect("verify_attempts mutex poisoned");
    matches!(
        attempts.get(&user_id),
        Some((count, window_start))
            if *count >= MAX_FAILURES && window_start.elapsed() < ATTEMPT_WINDOW
    )
}

/// Record one failed verification attempt for `user_id`, starting a fresh window if
/// the previous one has elapsed (or none existed).
fn record_failure(attempts: &AttemptMap, user_id: UserId) {
    let mut attempts = attempts.lock().expect("verify_attempts mutex poisoned");
    match attempts.get_mut(&user_id) {
        Some(entry) if entry.1.elapsed() < ATTEMPT_WINDOW => entry.0 += 1,
        _ => {
            attempts.insert(user_id, (1, Instant::now()));
        }
    }
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

/// Outcome of trying to link a Discord account to a student id.
enum LinkOutcome {
    /// The row now exists for this Discord id (freshly inserted, or already present).
    Linked,
    /// The student id is already held by a *different* Discord account; nothing was
    /// written. Carries that other Discord id so the caller can log the conflict.
    Refused { existing_discord_id: String },
}

/// Add to dsec_discord_members table.
///
/// Idempotent for a Discord id that is already recorded. Refuses when the student id
/// is already claimed by a *different* Discord account (SEC-19): the durable fix is a
/// `UNIQUE` constraint on `dsec_discord_members.student_id`, but that DDL needs a
/// duplicate sweep on live Supabase first (an owner step), so this application-level
/// check is the safety net until then.
async fn add_dsec_discord_table(
    data: &Data,
    student_id: &str,
    member_id: &String,
) -> Result<LinkOutcome, Error> {
    if member_recorded(data, member_id).await? {
        return Ok(LinkOutcome::Linked);
    }

    if let Some(existing_discord_id) = student_id_owner(data, student_id).await?
        && &existing_discord_id != member_id
    {
        return Ok(LinkOutcome::Refused { existing_discord_id });
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

    Ok(LinkOutcome::Linked)
}

/// The Discord id currently linked to `student_id`, if any.
async fn student_id_owner(data: &Data, student_id: &str) -> Result<Option<String>, Error> {
    let rows: Vec<DiscordMemberRow> = data
        .state
        .supabase
        .database()
        .from("dsec_discord_members")
        .select("student_id,discord_id")
        .eq("student_id", student_id)
        .execute()
        .await?;
    Ok(rows.into_iter().next().map(|row| row.discord_id))
}

/// Link the account, assign the verified role, and send the success response.
///
/// If the student id is already claimed by a different Discord account the link is
/// refused: the caller sees the generic failure embed and the conflict is logged to
/// the logs channel with both Discord ids (never the student id).
async fn grant_verified_role(
    ctx: &Context,
    data: &Data,
    modal_submit: &ModalInteraction,
    discord_member: &Member,
    student_id: &str,
    verified_role_id: RoleId,
) -> Result<(), Error> {
    let member_id = discord_member.user.id.to_string();

    match add_dsec_discord_table(data, student_id, &member_id).await? {
        LinkOutcome::Refused { existing_discord_id } => {
            log_link_conflict(ctx, data, &member_id, &existing_discord_id).await;
            modal_submit
                .create_response(ctx, ephemeral_embed(verification_failed_embed()))
                .await?;
        }
        LinkOutcome::Linked => {
            discord_member.add_role(ctx, verified_role_id).await?;

            let embed = CreateEmbed::new().title("Verified ✅").description(format!(
                "You have been assigned the <@&{}> role!",
                verified_role_id
            ));
            modal_submit
                .create_response(ctx, ephemeral_embed(embed))
                .await?;
        }
    }
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

/// Log a failed verification to the logs channel. Records the Discord user id, a
/// fixed reason category and a timestamp — NEVER the submitted name or student id.
/// The logs channel is read by humans and persists forever, so nothing a member
/// typed into the modal may go here (SEC-19). `reason` is always a fixed literal.
async fn log_verification_failure(ctx: &Context, data: &Data, user_id: UserId, reason: &str) {
    let _ = log_embed(
        ctx,
        data.state.logs_channel_id,
        Some("Verification failed".to_string()),
        None,
        Some(format!("User <@{user_id}> (id `{user_id}`) — {reason}.")),
        None,
        None,
        None,
        None,
        Some(true),
    )
    .await;
}

/// Log a student-id link conflict to the logs channel with BOTH Discord ids and no
/// student id (SEC-19): someone tried to verify with a student id already linked to
/// a different Discord account.
async fn log_link_conflict(
    ctx: &Context,
    data: &Data,
    attempting_discord_id: &str,
    existing_discord_id: &str,
) {
    let _ = log_embed(
        ctx,
        data.state.logs_channel_id,
        Some("Verification refused: student id already linked".to_string()),
        None,
        Some(format!(
            "<@{attempting_discord_id}> (id `{attempting_discord_id}`) tried to verify with a student id already linked to <@{existing_discord_id}> (id `{existing_discord_id}`)."
        )),
        None,
        None,
        None,
        None,
        Some(true),
    )
    .await;
}

// TODO(SEC-19 follow-up): name + student id is not proof of ownership — both are
// semi-public, so anyone who knows a classmate's name and id can verify as them.
// The real fix is a possession proof: email a one-time code to the address on the
// roster and require it back. dsec-app already owns OTP machinery, so the cheap
// version is this bot calling dsec-api rather than growing its own email sender.
// That is feature-sized work, tracked separately, not a patch to this handler.
//
// Owner-only DB step (NOT done here, needs a maintainer on live Supabase): sweep
// for duplicates, then add `UNIQUE` on dsec_discord_members.student_id. See
// SECURITY.md. The uniqueness check below is the application-level safety net until
// that constraint exists.
//
/// Handle a click on the "verify" button: collect the modal, then verify the
/// submitted student id/name against the database on every attempt.
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
        // SEC-19: cap failed attempts before ANY database work. A user who has
        // already failed too many times in the window gets the generic failure
        // embed and no query runs.
        if is_rate_limited(&data.state.verify_attempts, user_id) {
            log_verification_failure(ctx, data, user_id, "rate-limited").await;
            modal_submit
                .create_response(ctx, ephemeral_embed(verification_failed_embed()))
                .await?;
            return Ok(());
        }

        let student_id = normalise_student_id(&modal_data.student_id);

        // `fetch_student` is the only query carrying `membership_status = "Active"`,
        // and it now runs on every attempt before any role grant (SEC-19): no cache
        // shortcut can admit a member whose membership has since lapsed.
        let Some(student) = fetch_student(data, &student_id).await? else {
            record_failure(&data.state.verify_attempts, user_id);
            log_verification_failure(ctx, data, user_id, "no matching active membership").await;
            modal_submit
                .create_response(ctx, ephemeral_embed(verification_failed_embed()))
                .await?;
            return Ok(());
        };

        if name_matches(&student.full_name, &modal_data.name) {
            // Only fetch the guild member once we know we are about to grant the role.
            let discord_member = GuildId::member(guild_id, ctx, user_id).await?;
            grant_verified_role(
                ctx,
                data,
                &modal_submit,
                &discord_member,
                &student_id,
                verified_role_id,
            )
            .await?;
        } else {
            record_failure(&data.state.verify_attempts, user_id);
            log_verification_failure(ctx, data, user_id, "name mismatch").await;
            modal_submit
                .create_response(ctx, ephemeral_embed(verification_failed_embed()))
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

    #[test]
    fn rate_limits_after_max_failures() {
        let attempts: AttemptMap = Mutex::new(HashMap::new());
        let user = UserId::new(1);

        // A fresh user is never limited.
        assert!(!is_rate_limited(&attempts, user));

        // The first MAX_FAILURES attempts are allowed through (they still hit the DB).
        for _ in 0..MAX_FAILURES {
            assert!(!is_rate_limited(&attempts, user));
            record_failure(&attempts, user);
        }

        // The next attempt (the 6th, with MAX_FAILURES == 5) is refused with no query.
        assert!(is_rate_limited(&attempts, user));

        // A different user is unaffected.
        assert!(!is_rate_limited(&attempts, UserId::new(2)));
    }
}
