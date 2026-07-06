use std::time::Duration;

use crate::{
    Data, Error,
    commands::verification::{StudentRow, VerificationModal},
};
use ::serenity::all::{
    Context, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, GuildId, RoleId,
    collector::ModalInteractionCollector,
};
use dotenv::dotenv;
use poise::Modal as _;
use poise::serenity_prelude as serenity;

pub async fn on_interaction_create(
    ctx: &Context,
    interaction: &serenity::Interaction,
    data: &Data,
) -> Result<(), Error> {
    let Some(component_interaction) = interaction.as_message_component() else {
        return Ok(());
    };

    if component_interaction.data.custom_id == "verify" {
        dotenv().ok();
        let guild_id = match component_interaction.guild_id {
            Some(id) => id,
            None => {
                component_interaction.create_response(ctx, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .add_embed(CreateEmbed::new()
                            .title("Unable to perform action")
                            .description("Action can only be performed in the DSEC server"))
                        .ephemeral(true),
                )).await?;
                return Ok(());
            }
        };

        let role_id_string = std::env::var("VERIFIED_ROLE_ID").expect("missing VERIFIED_ROLE_ID");
        let role_id_u64: u64 = role_id_string
            .parse()
            .expect("Unable to parse VERIFIED_ROLE_ID into number");

        let user_id = component_interaction.user.id;
        let verified_role_id = RoleId::new(role_id_u64);
        let discord_member = GuildId::member(guild_id, ctx, user_id).await?;

        if discord_member.roles.contains(&verified_role_id) {
            component_interaction.create_response(ctx, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(CreateEmbed::new()
                        .title("Already Verified ✅")
                        .description(format!("You already have the <@&{}> role!", verified_role_id)))
                    .ephemeral(true),
            )).await?;
            return Ok(());
        }

        let modal_custom_id = component_interaction.id.to_string();

        let modal = VerificationModal::create(None, modal_custom_id.clone());
        component_interaction.create_response(ctx, modal).await?;

        let modal_submit = ModalInteractionCollector::new(&ctx.shard)
            .filter(move |d| d.data.custom_id == modal_custom_id)
            .timeout(Duration::from_secs(120))
            .await;

        let modal_submit = match modal_submit {
            Some(x) => x,
            None => return Ok(()),
        };

        let modal_data = match VerificationModal::parse(modal_submit.data.clone()) {
            Ok(data) => data,
            Err(_) => {
                modal_submit.create_response(ctx, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .add_embed(CreateEmbed::new()
                            .title("Error")
                            .description("Failed to parse modal data. Please try again."))
                        .ephemeral(true),
                )).await?;
                return Ok(());
            }
        };

        let input_student_id: &str = &modal_data.student_id.to_lowercase();
        let student_id = input_student_id
            .strip_prefix("s")
            .unwrap_or(input_student_id);

        let state = &data.state;

        let student_in_cache: bool = {
            let cache = state.student_cache.lock().expect("Failed to get cache");
            match cache.get(student_id) {
                Some(cached_name) => cached_name == &modal_data.name.to_lowercase(),
                None => false,
            }
        };

        if student_in_cache {
            discord_member.add_role(ctx, verified_role_id).await?;

            let embed = CreateEmbed::new()
                .title("Verified ✅")
                .description(format!(
                    "You have been assigned the <@&{}> role!",
                    verified_role_id
                ))
                .footer(CreateEmbedFooter::new("⚡ via cache"));

            modal_submit.create_response(ctx, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(embed)
                    .ephemeral(true),
            )).await?;

            return Ok(());
        }

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
            modal_submit.create_response(ctx, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(CreateEmbed::new()
                        .title("Student ID not found!")
                        .description("Your student ID is not found.\nIt takes up to **a week** for your membership to be updated in the database since sign up.\nTry again later."))
                    .ephemeral(true),
            )).await?;
            return Ok(());
        }

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

            let embed = CreateEmbed::new()
                .title("Verified ✅")
                .description(format!(
                    "You have been assigned the <@&{}> role!",
                    verified_role_id
                ));

            modal_submit.create_response(ctx, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(embed)
                    .ephemeral(true),
            )).await?;
        } else {
            modal_submit.create_response(ctx, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(CreateEmbed::new()
                        .title("Name mismatch ❌")
                        .description("Your student ID is present, however the name does not match. Try again."))
                    .ephemeral(true),
            )).await?;
        }
    }

    Ok(())
}
