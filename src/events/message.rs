use crate::Error;
use dotenv::dotenv;
use poise::serenity_prelude as serenity;
use ::serenity::model::id::{ChannelId, GuildId};

pub async fn on_message(
    ctx: &serenity::Context,
    new_message: &serenity::Message
) -> Result<(), Error> {
    dotenv().ok();
    // check if it's the honeypot channel
    let discord_guild_id = std::env::var("GUILD_ID").expect("missing GUILD_ID").parse::<u64>().expect("Invalid GUILD_ID value");

    let honeypot_channel_id_env = std::env::var("HONEYPOT_CHANNEL_ID")
        .expect("missing HONEYPOT_CHANNEL_ID")
        .parse::<u64>()
        .expect("Invalid HONEYPOT_CHANNEL_ID value");
    let honeypot_channel_id = ChannelId::new(honeypot_channel_id_env);
    let current_channel_id = &new_message.channel_id;

    if honeypot_channel_id.eq(current_channel_id) {
        let user = &new_message.author;
        let ban_user = GuildId::new(discord_guild_id).ban_with_reason(ctx, user, 2, "Message sent in honeypot channel.").await;
        ban_user.expect("Failed to ban user");
    }

    Ok(())
}
