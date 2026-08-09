use crate::{Error, commands::mods_only::log_embed};
use ::serenity::model::id::{ChannelId, GuildId};
use dotenv::dotenv;
use poise::serenity_prelude as serenity;

async fn honeypot(ctx: &serenity::Context, new_message: &serenity::Message) -> Result<(), Error> {
    dotenv().ok();
    // check if it's the honeypot channel
    let discord_guild_id = std::env::var("GUILD_ID")
        .expect("missing GUILD_ID")
        .parse::<u64>()
        .expect("Invalid GUILD_ID value");

    let honeypot_channel_id_env = std::env::var("HONEYPOT_CHANNEL_ID")
        .expect("missing HONEYPOT_CHANNEL_ID")
        .parse::<u64>()
        .expect("Invalid HONEYPOT_CHANNEL_ID value");
    let honeypot_channel_id = ChannelId::new(honeypot_channel_id_env);
    let current_channel_id = &new_message.channel_id;

    if honeypot_channel_id.eq(current_channel_id) {
        let user = &new_message.author;
        let username = &user.name;
        let user_id = &user.id;
        let avatar_url = user.avatar_url();
        let ban_user = GuildId::new(discord_guild_id)
            .ban_with_reason(ctx, user_id, 2, "Message sent in honeypot channel.")
            .await;

        if ban_user.is_ok() {
            log_embed(
                ctx,
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
) -> Result<(), Error> {
    dotenv().ok();

    let leetcode_channel_id_env = std::env::var("LEETCODE_CHANNEL_ID")
        .expect("missing LEETCODE_CHANNEL_ID")
        .parse::<u64>()
        .expect("Invalid LEETCODE_CHANNEL_ID value");

    let leetcode_channel_id = ChannelId::new(leetcode_channel_id_env);

    let current_channel_id = &new_message.channel_id;

    if leetcode_channel_id.eq(current_channel_id) {
        fn create_title_from_message(message: impl Into<String>) -> String {
            let message_string: String = message.into();

            let title = message_string
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
                .to_string();
            title
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
) -> Result<(), Error> {
    let _ = honeypot(ctx, new_message).await?;
    let _ = create_leetcode_thread(ctx, new_message).await?;
    Ok(())
}
