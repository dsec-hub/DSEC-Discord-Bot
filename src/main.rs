use dotenv::dotenv;
use poise::serenity_prelude as serenity;
use std::{collections::HashMap, sync::Mutex};
use supabase::prelude::Client;
mod commands;
mod events;

#[derive(Debug)]
pub struct Data {
    pub state: AppState,
}

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;
type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

#[derive(Debug)]
pub struct AppState {
    pub supabase: Client,
    pub student_cache: Mutex<HashMap<String, String>>,
    // Parsed once at boot. Re-reading these per event means a config typo takes
    // down a handler at some random future moment instead of failing the deploy.
    pub guild_id: serenity::GuildId,
    pub honeypot_channel_id: serenity::ChannelId,
    pub leetcode_channel_id: serenity::ChannelId,
    pub verified_role_id: serenity::RoleId,
    pub logs_channel_id: serenity::ChannelId,
    // Optional: only /weather uses it. Read once here so no handler re-reads the
    // process environment, but a missing value must not stop the bot from booting.
    pub weather_token: String,
}

/// Read a required `u64` snowflake from the environment, recording the variable
/// name in `missing` (rather than panicking on the first one) so every offending
/// variable can be reported together.
fn required_u64(name: &str, missing: &mut Vec<String>) -> u64 {
    match std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(v) => v,
        None => {
            missing.push(name.to_string());
            0
        }
    }
}

impl AppState {
    // The large `Err` variant is `supabase::Error` from the `supabase-lib-rs` crate;
    // boxing it would change this public signature rather than shrink their type.
    #[allow(clippy::result_large_err)]
    pub async fn new() -> supabase::Result<Self> {
        dotenv().ok();

        // Parse and validate every required id up front, collecting all failures
        // into one actionable startup error instead of dying on the first one.
        let mut missing: Vec<String> = Vec::new();
        let guild_id = required_u64("GUILD_ID", &mut missing);
        let honeypot_channel_id = required_u64("HONEYPOT_CHANNEL_ID", &mut missing);
        let leetcode_channel_id = required_u64("LEETCODE_CHANNEL_ID", &mut missing);
        let verified_role_id = required_u64("VERIFIED_ROLE_ID", &mut missing);
        let logs_channel_id = required_u64("LOGS_CHANNEL_ID", &mut missing);
        if !missing.is_empty() {
            eprintln!(
                "Missing or unparseable environment variables: {}.\nCopy .env.example to .env and fill them in.",
                missing.join(", ")
            );
            std::process::exit(1);
        }

        // Non-fatal: the bot boots without a weather key, /weather just fails.
        let weather_token = std::env::var("WEATHER_TOKEN").unwrap_or_default();

        let supabase_url = std::env::var("SUPABASE_URL").expect("missing SUPABASE_URL");
        let supabase_key = std::env::var("SUPABASE_KEY").expect("missing SUPABASE_KEY");
        let supabase_user_email =
            std::env::var("SUPABASE_USER_EMAIL").expect("missing SUPABASE_USER_EMAIL");
        let supabase_user_password =
            std::env::var("SUPABASE_USER_PASSWORD").expect("missing SUPABASE_USER_PASSWORD");
        let client = Client::new(&supabase_url, &supabase_key)?;

        match client
            .auth()
            .sign_in_with_email_and_password(&supabase_user_email, &supabase_user_password)
            .await
        {
            Ok(auth_response) => match auth_response.user.and_then(|user| user.email) {
                Some(email) => println!("User signed in: {email}"),
                None => println!("User not found"),
            },
            Err(err) => {
                eprintln!("Failed to connect/sign in to Supabase, continuing setup anyways: {err}");
            }
        }

        Ok(Self {
            supabase: client,
            student_cache: Mutex::new(HashMap::new()),
            guild_id: serenity::GuildId::new(guild_id),
            honeypot_channel_id: serenity::ChannelId::new(honeypot_channel_id),
            leetcode_channel_id: serenity::ChannelId::new(leetcode_channel_id),
            verified_role_id: serenity::RoleId::new(verified_role_id),
            logs_channel_id: serenity::ChannelId::new(logs_channel_id),
            weather_token,
        })
    }
}

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            events::ready::on_ready(ctx, data_about_bot).await?;
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            events::interaction_create::on_interaction_create(ctx, interaction, data).await?;
        }
        serenity::FullEvent::Message { new_message } => {
            events::message::on_message(ctx, new_message, data).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Framework-level error handler. On a slash-command error it replies ephemerally
/// so the user is not left with a dead interaction, and it always runs poise's
/// default logging. `FrameworkOptions::default()` installs this default logger
/// too, but nothing surfaces it (no tracing subscriber, no log shipping — OPS-04),
/// so an explicit handler is set (COR-03).
async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    if let poise::FrameworkError::Command { ctx, .. } = &error {
        let _ = ctx
            .send(
                poise::CreateReply::default()
                    .content(
                        "Something went wrong — a maintainer has been notified. Please try again in a minute.",
                    )
                    .ephemeral(true),
            )
            .await;
    }
    if let Err(e) = poise::builtins::on_error(error).await {
        eprintln!("[on_error] failed while handling a framework error: {e}");
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok(); // load env

    let app_state = AppState::new()
        .await
        .expect("Failed to initialize AppState");

    // -- discord bot start --
    let token = std::env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::info::help(),
                commands::info::ping(),
                commands::info::userinfo(),
                commands::info::serverinfo(),
                commands::info::botinfo(),
                commands::weather::weather(),
                commands::verification::verify(),
                commands::mods_only::embed(),
                commands::member_info::member_info(),
            ],
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            on_error: |error| Box::pin(on_error(error)),

            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data { state: app_state })
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client
        .expect("Client failed to start")
        .start()
        .await
        .expect("Client failed to start");

    // -- discord bot end --
}
