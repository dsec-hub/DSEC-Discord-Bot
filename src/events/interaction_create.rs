use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    Data, Error, redact_digits,
    commands::{
        mods_only::log_embed,
        verification::{StudentRow, VerificationModal},
    },
};
use ::serenity::{
    all::{
        ComponentInteraction, Context, CreateEmbed, CreateInteractionResponse,
        CreateInteractionResponseMessage, EditInteractionResponse, GuildId, ModalInteraction,
        RoleId, UserId, collector::ModalInteractionCollector,
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
///
/// Recovers a poisoned lock rather than panicking: a poisoned `verify_attempts`
/// mutex must not turn every future verification into a panic (the map holds only
/// counters, never invariant-critical state).
fn is_rate_limited(attempts: &AttemptMap, user_id: UserId) -> bool {
    let attempts = attempts.lock().unwrap_or_else(|p| p.into_inner());
    matches!(
        attempts.get(&user_id),
        Some((count, window_start))
            if *count >= MAX_FAILURES && window_start.elapsed() < ATTEMPT_WINDOW
    )
}

/// Record one failed verification attempt for `user_id`. Fully-elapsed windows are
/// evicted first, which both resets a returning user's window and bounds the map so
/// it cannot grow without limit.
fn record_failure(attempts: &AttemptMap, user_id: UserId) {
    let mut attempts = attempts.lock().unwrap_or_else(|p| p.into_inner());
    attempts.retain(|_, (_, window_start)| window_start.elapsed() < ATTEMPT_WINDOW);
    match attempts.get_mut(&user_id) {
        // Any surviving entry is within the window (retain kept it), so increment.
        Some(entry) => entry.0 += 1,
        None => {
            attempts.insert(user_id, (1, Instant::now()));
        }
    }
}

/// Get (or create) the per-user attempt lock. The std mutex guarding the map is
/// released before the caller awaits the returned tokio lock, so no std guard is
/// ever held across an await. Entries no attempt is using any more (only the map
/// still references them) are pruned so the map cannot grow without bound.
fn user_attempt_lock(data: &Data, user_id: UserId) -> Arc<AsyncMutex<()>> {
    let mut locks = data
        .state
        .verify_locks
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    Arc::clone(
        locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
    )
}

/// Edit the deferred ephemeral response for a modal submission (SEC-19 #7: we
/// `defer_ephemeral` the moment the modal arrives, so every later reply is an edit).
async fn edit_reply(
    ctx: &Context,
    modal_submit: &ModalInteraction,
    embed: CreateEmbed,
) -> Result<(), Error> {
    modal_submit
        .edit_response(ctx, EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

/// Whether a supabase error is a Postgres unique-constraint violation (SQLSTATE
/// 23505). Used only as a boolean signal — the error string embeds the student id
/// (`Key (student_id)=(…)`) and must never be logged (SEC-19 #4).
fn is_unique_violation(err: &supabase::Error) -> bool {
    err.to_string().contains("23505")
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
    /// The row now exists for this Discord id (freshly inserted, or already present
    /// with exactly this student id — an idempotent re-verify).
    Linked,
    /// The submitted student id is already held by a *different* Discord account;
    /// nothing was written. Carries that other Discord id for the conflict audit.
    RefusedIdClaimed { existing_discord_id: String },
    /// This caller's Discord account is already linked to a *different* student id, so
    /// it may not claim another. The stale-row / hijack path (SEC-19 #2).
    RefusedCallerLinked,
}

/// Add to dsec_discord_members table.
///
/// The ownership of the *submitted* id is always checked before anything is written
/// (SEC-19 #2): an idempotent re-verify is allowed only when this caller already owns
/// exactly that id. A caller already linked to a *different* id is refused, closing
/// the path where a stale COR-03 partial-insert row let an account claim someone
/// else's id. A concurrent-insert `UNIQUE` violation (23505) is caught and converted
/// to the same refusal (SEC-19 #3) — the constraint itself is an owner/deploy step.
async fn add_dsec_discord_table(
    data: &Data,
    student_id: &str,
    member_id: &String,
) -> Result<LinkOutcome, Error> {
    // 1. Who owns the SUBMITTED id right now? Always check before granting anything.
    if let Some(existing_discord_id) = student_id_owner(data, student_id).await? {
        if &existing_discord_id == member_id {
            return Ok(LinkOutcome::Linked); // idempotent: caller already holds this id
        }
        return Ok(LinkOutcome::RefusedIdClaimed { existing_discord_id });
    }

    // 2. Submitted id is unowned. If this caller already holds a DIFFERENT id, refuse:
    //    a linked account trying to claim a new student id is the hijack / stale-row path.
    if member_recorded(data, member_id).await? {
        return Ok(LinkOutcome::RefusedCallerLinked);
    }

    // 3. Insert. `.returning(...)` makes supabase-lib-rs send `Prefer:
    //    return=representation`; without it PostgREST answers a POST with an empty
    //    `return=minimal` body that fails to deserialise, aborting before the role
    //    grant even though the row was written (COR-03).
    let new_member = serde_json::json!({
        "student_id": student_id,
        "discord_id": member_id,
    });
    let insert: supabase::Result<Vec<DiscordMemberRow>> = data
        .state
        .supabase
        .database()
        .insert("dsec_discord_members")
        .values(new_member)?
        .returning("student_id,discord_id")
        .execute()
        .await;

    match insert {
        Ok(_) => Ok(LinkOutcome::Linked),
        // A UNIQUE(student_id) violation means another account inserted the same id
        // between our check and our insert. Re-query the owner and refuse — never
        // surface the raw 23505 body, which echoes the student id (SEC-19 #3, #4).
        Err(err) if is_unique_violation(&err) => {
            match student_id_owner(data, student_id).await? {
                Some(owner) if &owner != member_id => {
                    Ok(LinkOutcome::RefusedIdClaimed { existing_discord_id: owner })
                }
                // The winning row is ours (or has since vanished): treat as linked.
                _ => Ok(LinkOutcome::Linked),
            }
        }
        Err(err) => Err(err.into()),
    }
}

/// The Discord id currently linked to `student_id`, if any. Selects only `discord_id`
/// (minimum columns — SEC-19 #4).
async fn student_id_owner(data: &Data, student_id: &str) -> Result<Option<String>, Error> {
    #[derive(Deserialize)]
    struct OwnerRow {
        discord_id: String,
    }
    let rows: Vec<OwnerRow> = data
        .state
        .supabase
        .database()
        .from("dsec_discord_members")
        .select("discord_id")
        .eq("student_id", student_id)
        .execute()
        .await?;
    Ok(rows.into_iter().next().map(|row| row.discord_id))
}

/// A name-matched attempt: try to link the account and grant the role.
///
/// Returns `true` when the role was granted, `false` when the link was refused (an
/// ownership conflict or a caller already linked to a different id). On refusal the
/// caller records exactly one failed attempt (SEC-19 #6); the user sees the generic
/// embed either way. All responses edit the deferred ephemeral (SEC-19 #7).
async fn link_and_grant(
    ctx: &Context,
    data: &Data,
    modal_submit: &ModalInteraction,
    discord_member: &Member,
    student_id: &str,
    verified_role_id: RoleId,
) -> Result<bool, Error> {
    let member_id = discord_member.user.id.to_string();

    match add_dsec_discord_table(data, student_id, &member_id).await? {
        LinkOutcome::Linked => {
            discord_member.add_role(ctx, verified_role_id).await?;
            let embed = CreateEmbed::new().title("Verified ✅").description(format!(
                "You have been assigned the <@&{}> role!",
                verified_role_id
            ));
            edit_reply(ctx, modal_submit, embed).await?;
            Ok(true)
        }
        LinkOutcome::RefusedIdClaimed { existing_discord_id } => {
            log_link_conflict(ctx, data, &member_id, &existing_discord_id).await;
            edit_reply(ctx, modal_submit, verification_failed_embed()).await?;
            Ok(false)
        }
        LinkOutcome::RefusedCallerLinked => {
            log_caller_already_linked(ctx, data, &member_id).await;
            edit_reply(ctx, modal_submit, verification_failed_embed()).await?;
            Ok(false)
        }
    }
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

/// Post an audit line to the logs channel: a fixed title, the Discord user id and a
/// timestamp only — NEVER the submitted name or student id. The logs channel is read
/// by humans and persists forever, so nothing a member typed into the modal may go
/// here (SEC-19). `title` and `body` are always fixed literals.
async fn log_verify_audit(ctx: &Context, data: &Data, title: &str, body: String) {
    let _ = log_embed(
        ctx,
        data.state.logs_channel_id,
        Some(title.to_string()),
        None,
        Some(body),
        None,
        None,
        None,
        None,
        Some(true),
    )
    .await;
}

/// Log a failed verification attempt. The description is one fixed generic string:
/// "student id not found" and "name mismatch" must be indistinguishable in the log
/// too, so it cannot become a mod-visible membership oracle (SEC-19 #8).
async fn log_verification_failure(ctx: &Context, data: &Data, user_id: UserId) {
    log_verify_audit(
        ctx,
        data,
        "Verification failed",
        format!("User <@{user_id}> (id `{user_id}`) — a verification attempt failed."),
    )
    .await;
}

/// Log that a user was refused because they are already at the attempt limit.
async fn log_rate_limited(ctx: &Context, data: &Data, user_id: UserId) {
    log_verify_audit(
        ctx,
        data,
        "Verification rate-limited",
        format!("User <@{user_id}> (id `{user_id}`) — too many attempts; refused without a database query."),
    )
    .await;
}

/// Log that a caller already linked to a *different* student id tried to claim a new
/// one. Carries only the caller's Discord id — never a student id (SEC-19 #2).
async fn log_caller_already_linked(ctx: &Context, data: &Data, member_id: &str) {
    log_verify_audit(
        ctx,
        data,
        "Verification refused: account already linked",
        format!(
            "<@{member_id}> (id `{member_id}`) is already linked to a different student id and tried to claim another; refused."
        ),
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
    log_verify_audit(
        ctx,
        data,
        "Verification refused: student id already linked",
        format!(
            "<@{attempting_discord_id}> (id `{attempting_discord_id}`) tried to verify with a student id already linked to <@{existing_discord_id}> (id `{existing_discord_id}`)."
        ),
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

    let user_id = component_interaction.user.id;

    // Acknowledge the modal submission within Discord's ~3s window BEFORE any database
    // or logs-channel work, so a slow query can never leave a dead "This interaction
    // failed" and a mutation can never happen with no ack (SEC-19 #7). Every later
    // reply edits this deferred ephemeral response.
    if let Err(err) = modal_submit.defer_ephemeral(ctx).await {
        eprintln!(
            "[verify] defer failed for interaction {}: {}",
            modal_submit.id,
            redact_digits(&err.to_string())
        );
    }

    // Serialize all verification work for THIS user, held across the whole attempt, so
    // concurrent modal submits cannot each slip under the attempt limit or the
    // uniqueness checks (SEC-19 #5). Acquired AFTER defer so waiting on it never eats
    // the ack window; different users never contend.
    let attempt_lock = user_attempt_lock(data, user_id);
    let _attempt_guard = attempt_lock.lock().await;

    // Any failure inside still edits the deferred response rather than leaving the user
    // stuck — poise's on_error cannot reach this modal submission (COR-03).
    let verify_result: Result<(), Error> = async {
        // Cap failed attempts before ANY database work (SEC-19 #3): a user already over
        // the limit gets the generic embed and no query runs.
        if is_rate_limited(&data.state.verify_attempts, user_id) {
            log_rate_limited(ctx, data, user_id).await;
            edit_reply(ctx, &modal_submit, verification_failed_embed()).await?;
            return Ok(());
        }

        let student_id = normalise_student_id(&modal_data.student_id);

        // `fetch_student` is the only query carrying `membership_status = "Active"`,
        // and it runs on every attempt before any role grant (SEC-19): no cache
        // shortcut can admit a member whose membership has since lapsed.
        let Some(student) = fetch_student(data, &student_id).await? else {
            record_failure(&data.state.verify_attempts, user_id);
            log_verification_failure(ctx, data, user_id).await;
            edit_reply(ctx, &modal_submit, verification_failed_embed()).await?;
            return Ok(());
        };

        if name_matches(&student.full_name, &modal_data.name) {
            // Fetch the guild member only once we know we may grant the role.
            let discord_member = GuildId::member(guild_id, ctx, user_id).await?;
            let granted = link_and_grant(
                ctx,
                data,
                &modal_submit,
                &discord_member,
                &student_id,
                verified_role_id,
            )
            .await?;
            // A refused link (ownership conflict, or a caller already linked to a
            // different id) counts as exactly one failed attempt (SEC-19 #6); a
            // successful grant counts none.
            if !granted {
                record_failure(&data.state.verify_attempts, user_id);
            }
        } else {
            record_failure(&data.state.verify_attempts, user_id);
            log_verification_failure(ctx, data, user_id).await;
            edit_reply(ctx, &modal_submit, verification_failed_embed()).await?;
        }

        Ok(())
    }
    .await;

    if let Err(err) = verify_result {
        // Never print the raw error on the verify path: supabase/reqwest errors embed
        // the PostgREST URL (…student_id=eq.<id>) and a 23505 body echoes the id, both
        // PII. Redact digit runs; the interaction id is the correlation ref (SEC-19 #4).
        eprintln!(
            "[verify] interaction {} failed: {}",
            modal_submit.id,
            redact_digits(&err.to_string())
        );
        let _ = edit_reply(
            ctx,
            &modal_submit,
            CreateEmbed::new().title("Something went wrong").description(format!(
                "A maintainer has been notified. Please try again in a minute. (ref: {})",
                modal_submit.id
            )),
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

    #[test]
    fn redacts_ids_but_keeps_short_numbers() {
        // Student id (9 digits) and Discord snowflake (19) are masked.
        assert_eq!(
            redact_digits("student_id=eq.123456789 for <@1234567890123456789>"),
            "student_id=eq.<redacted> for <@<redacted>>"
        );
        // A 23505 key detail is masked.
        assert_eq!(
            redact_digits("Key (student_id)=(220123456) already exists"),
            "Key (student_id)=(<redacted>) already exists"
        );
        // Short runs (< 7 digits, e.g. the SQLSTATE code itself) are preserved.
        assert_eq!(redact_digits("code 23505"), "code 23505");
        assert_eq!(redact_digits("no digits here"), "no digits here");
    }
}
