use crate::{Data, Error, commands::mods_only::log_embed};
use poise::serenity_prelude as serenity;

async fn honeypot(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    // Config is parsed once at boot and stored on AppState (COL-BOT-05).
    let honeypot_channel_id = data.state.honeypot_channel_id;
    let current_channel_id = &new_message.channel_id;

    if honeypot_channel_id.eq(current_channel_id) {
        let user = &new_message.author;
        let username = &user.name;
        let user_id = &user.id;
        let avatar_url = user.avatar_url();
        let ban_user = data
            .state
            .guild_id
            .ban_with_reason(ctx, user_id, 2, "Message sent in honeypot channel.")
            .await;

        if ban_user.is_ok() {
            log_embed(
                ctx,
                data.state.logs_channel_id,
                Some("Honeypot activated!".to_string()),
                None,
                Some(format!("User got banned: {}", username)),
                Some(format!("ID: {}", user_id)),
                None,
                avatar_url,
                None,
                Some(true),
            )
            .await?;
        }
    }

    Ok(())
}

async fn create_leetcode_thread(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    let leetcode_channel_id = data.state.leetcode_channel_id;

    let current_channel_id = &new_message.channel_id;

    if leetcode_channel_id.eq(current_channel_id) {
        fn create_title_from_message(message: impl Into<String>) -> String {
            let message_string: String = message.into();

            message_string
                .lines()
                .next()
                .map(|line| {
                    let start = line
                        .char_indices()
                        .nth(2)
                        .map(|(i, _)| i)
                        .unwrap_or(line.len());
                    &line[start..]
                })
                .unwrap_or("")
                .to_string()
        }

        let new_thread =
            serenity::CreateThread::new(create_title_from_message(&new_message.content));
        current_channel_id
            .create_thread_from_message(ctx, new_message.id, new_thread)
            .await?;
    };

    Ok(())
}

pub async fn on_message(
    ctx: &serenity::Context,
    new_message: &serenity::Message,
    data: &Data,
) -> Result<(), Error> {
    honeypot(ctx, new_message, data).await?;
    create_leetcode_thread(ctx, new_message, data).await?;
    Ok(())
}
