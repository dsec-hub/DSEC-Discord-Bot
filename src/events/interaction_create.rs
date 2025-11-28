use std::time::Duration;

use crate::{
    Data, Error,
    commands::verification::{StudentRow, VerificationModal},
};
use ::serenity::all::{
    ComponentInteraction, Context, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, GuildId, RoleId,
};
use dotenv::dotenv;
use poise::{modal, serenity_prelude as serenity};

struct ContextRef<'a>(&'a Context);
impl AsRef<Context> for ContextRef<'_> {
    fn as_ref(&self) -> &Context {
        self.0
    }
}

async fn embed_response(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    title: impl Into<String>,
    description: impl Into<String>,
) -> Result<(), Error> {
    let embed = CreateEmbed::new().title(title).description(description);

    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .add_embed(embed)
            .ephemeral(true),
    );

    interaction.create_response(ctx, response).await?;

    Ok(())
}

async fn embed_followup(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    title: impl Into<String>,
    description: impl Into<String>,
) -> Result<(), Error> {
    let embed = CreateEmbed::new().title(title).description(description);

    let response = CreateInteractionResponseFollowup::new()
        .add_embed(embed)
        .ephemeral(true);

    interaction.create_followup(ctx, response).await?;

    Ok(())
}

pub async fn on_interaction_create(
    ctx: &serenity::Context,
    interaction: &serenity::Interaction,
    data: &Data,
) -> Result<(), Error> {
    let Some(interaction) = interaction.as_message_component() else {
        return Ok(());
    };

    if interaction.data.custom_id == "verify" {
        dotenv().ok();
        let guild_id = match interaction.guild_id {
            Some(id) => id,
            None => {
                // user not in a server at all
                embed_response(
                    ctx,
                    interaction,
                    "Unable to perform action",
                    "Action can only be performed in the DSEC server",
                )
                .await?;

                return Ok(());
            }
        };

        let role_id_string = std::env::var("VERIFIED_ROLE_ID").expect("missing VERIFIED_ROLE_ID");

        let role_id_u64: u64 = role_id_string
            .parse()
            .expect("Unable to parse VERIFIED_ROLE_ID into number");

        let user_id = interaction.user.id;
        let verified_role_id = RoleId::new(role_id_u64);

        let discord_member = GuildId::member(guild_id, ctx, user_id).await?;
        let has_role = discord_member.roles.contains(&verified_role_id);

        // Has role
        if has_role {
            embed_response(
                ctx,
                interaction,
                "Already Verified ✅",
                format!("You already have the <@&{}> role!", verified_role_id),
            )
            .await?;

            return Ok(());
        }

        // modal
        let timeout = Duration::from_secs(120);

        let modal_data = modal::execute_modal_on_component_interaction::<VerificationModal>(
            ContextRef(ctx),
            interaction.clone(),
            None,
            Some(timeout),
        )
        .await?;

        let modal_data = match modal_data {
            Some(data) => data,
            None => return Ok(()),
        };

        let input_student_id: &str = &modal_data.student_id.to_lowercase();
        let student_id = input_student_id
            .strip_prefix("s")
            .unwrap_or(input_student_id);

        let state = &data.state;

        let student_in_cache: bool = {
            let cache: std::sync::MutexGuard<'_, std::collections::HashMap<String, String>> =
                state.student_cache.lock().expect("Failed to get cache");

            match cache.get(student_id) {
                Some(cached_name) => cached_name == &modal_data.name.to_lowercase(),
                None => false,
            }
        };

        if student_in_cache {
            discord_member.add_role(ctx, verified_role_id).await?;

            let verified_cache_embed = CreateEmbed::new()
                .title("Verified ✅")
                .description(format!(
                    "You have been assigned the <@&{}> role!",
                    verified_role_id
                ))
                .footer(CreateEmbedFooter::new("⚡ via cache"));

            let verified_msg = CreateInteractionResponseFollowup::new()
                .add_embed(verified_cache_embed)
                .ephemeral(true);

            interaction.create_followup(ctx, verified_msg).await?;

            return Ok(());
        }

        // fetch from DB
        let student_data: Vec<StudentRow> = state
            .supabase
            .database()
            .from("active_members")
            .select("full_name, student_id")
            .eq("student_id", &student_id)
            .execute()
            .await?;

        let result = student_data.iter().next();

        if result.is_none() {
            // TODO: add user to "don't use this command for 5 minutes"

            embed_followup(ctx, 
                interaction, 
                "Student ID not found!", 
                "Your student ID is not found. 
                It takes up to **a week** for your membership to be updated in the database since sign up. 
                Try again later.").await?;

            return Ok(());
        }

        // get name from result
        let result_name = &result.unwrap().full_name;

        {
            let mut cache = state.student_cache.lock().unwrap();
            cache.insert(
                student_id.to_string(),
                result_name.to_string().to_lowercase(),
            );
        }

        if &result_name.to_lowercase() == &modal_data.name.to_lowercase() {
            discord_member.add_role(ctx, verified_role_id).await?;

            embed_followup(
                ctx,
                interaction,
                "Verified ✅",
                format!("You have been assigned the <@&{}> role!", verified_role_id),
            )
            .await?;
        } else {
            embed_followup(
                ctx,
                interaction,
                "Name mismatch ❌",
                "Your student ID is present, however the name does not match. Try again.",
            )
            .await?;
        }
    }

    Ok(())
}
