use crate::Error;
use poise::serenity_prelude as serenity;

pub async fn on_ready(
    _ctx: &serenity::Context,
    data_about_bot: &serenity::Ready,
) -> Result<(), Error> {
    println!("Logged in as {}", data_about_bot.user.name);
    Ok(())
}
