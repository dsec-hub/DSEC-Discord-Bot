use crate::{ApplicationContext, Error};
use poise::{CreateReply, Modal};
use serde::{Deserialize, Serialize};
use serenity::all::{CreateActionRow, CreateButton, CreateEmbed};

#[derive(Deserialize, Serialize, Debug)]
pub struct StudentRow {
    pub full_name: String,
    pub student_id: String,
}

#[derive(Debug, Modal)]
#[name = "Club Verification"]
pub struct VerificationModal {
    #[name = "Full Name"]
    #[placeholder = "John Doe"]
    #[max_length = 50]
    pub name: String,
    #[name = "Student ID"]
    #[placeholder = "s123456789"]
    pub student_id: String,
}

/// Embed message with verify button to verify membership
#[poise::command(
    slash_command,
    required_permissions = "MANAGE_MESSAGES | MANAGE_THREADS"
)]
pub async fn verify(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let reply: CreateReply = {
        let embed: CreateEmbed = CreateEmbed::new()
            .title("Verify your DSEC membership")
            .description("Click **Verify Here** and enter your **Full name** and **Student ID** (e.g., s123456789). Your responses are private.");

        let button: CreateButton = CreateButton::new("verify").label("Verify Here");

        let components = vec![CreateActionRow::Buttons(vec![button])];

        CreateReply::default()
            .ephemeral(false)
            .embed(embed)
            .components(components)
    };

    ctx.send(reply).await?;

    Ok(())
}
