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

    let _: Vec<DiscordMemberRow> = data
        .state
        .supabase
        .database()
        .insert("dsec_discord_members")
        .values(new_member)?
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

/// Whether the cached name for `student_id` matches the submitted `name`.
fn cached_name_matches(data: &Data, student_id: &str, name: &str) -> bool {
    let cache = data
        .state
        .student_cache
        .lock()
        .expect("Failed to get cache");
    match cache.get(student_id) {
        Some(cached_name) => cached_name == &name.to_lowercase(),
        None => false,
    }
}

/// Store the resolved student name in the cache (lower-cased for comparison).
fn cache_student(data: &Data, student_id: &str, full_name: &str) {
    let mut cache = data.state.student_cache.lock().unwrap();
    cache.insert(student_id.to_string(), full_name.to_lowercase());
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

    // From here on we hold the modal-submit token, so the slower member fetch
    // and database work below is no longer racing the button's ack window.
    let user_id = component_interaction.user.id;
    let discord_member = GuildId::member(guild_id, ctx, user_id).await?;

    let input_student_id = modal_data.student_id.to_lowercase();
    let student_id = input_student_id
        .strip_prefix("s")
        .unwrap_or(&input_student_id);

    if cached_name_matches(data, student_id, &modal_data.name) {
        grant_verified_role(
            ctx,
            data,
            &modal_submit,
            &discord_member,
            student_id,
            verified_role_id,
            true,
        )
        .await?;
        return Ok(());
    }

    let Some(student) = fetch_student(data, student_id).await? else {
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

    cache_student(data, student_id, &student.full_name);

    if student.full_name.to_lowercase() == modal_data.name.to_lowercase() {
        grant_verified_role(
            ctx,
            data,
            &modal_submit,
            &discord_member,
            student_id,
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
