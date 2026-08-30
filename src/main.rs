use dotenv::dotenv;
use poise::serenity_prelude as serenity;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use supabase::prelude::Client;
mod commands;
mod events;

/// Mask any maximal run of 7+ ASCII digits so student ids (9 digits) and Discord
/// snowflakes (17-19 digits) never reach a log line. Supabase/reqwest error text
/// embeds the PostgREST URL (`…student_id=eq.<id>`) and a `23505` body echoes the id
/// in a `Key (…)=(…)` detail, so any error printed on a request path must be redacted
/// first (SEC-19). The 7-digit floor keeps short diagnostic codes (SQLSTATE like
/// `23505`, HTTP status) intact while masking every id length we actually hold. Names
/// never appear on these error paths (the failing queries filter by id only), and
/// correlation ids are printed separately, never through this function.
pub(crate) fn redact_digits(input: &str) -> String {
    const MIN_REDACTED_RUN: usize = 7;
    let mut out = String::with_capacity(input.len());
    let mut digits = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if digits.len() >= MIN_REDACTED_RUN {
            out.push_str("<redacted>");
        } else {
            out.push_str(&digits);
        }
        digits.clear();
        out.push(ch);
    }
    if digits.len() >= MIN_REDACTED_RUN {
        out.push_str("<redacted>");
    } else {
        out.push_str(&digits);
    }
    out
}

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
    // SEC-19: per-Discord-user failed-verification counter. `(failures, window_start)`
    // keyed by user id; once `failures` hits the limit inside the window the verify
    // handler refuses the attempt with no database round trip. The old
    // `student_cache` was removed: it was consulted *before* the only query carrying
    // the `membership_status = "Active"` filter, had no TTL and was never evicted, so
    // in a long-lived container it was a stale-membership bypass. One query per verify
    // is not a performance problem.
    pub verify_attempts: Mutex<HashMap<serenity::UserId, (u32, Instant)>>,
    // SEC-19: one async lock per Discord user, held across a whole verification
    // attempt so concurrent modal submits from the same user serialize and cannot each
    // slip under the attempt limit / uniqueness checks. A tokio Mutex (not std) because
    // the guard is held across awaits; the std Mutex here only guards the brief
    // get-or-insert of the map and is never held across an await. Idle entries are
    // pruned on access so the map cannot grow without bound.
    pub verify_locks: Mutex<HashMap<serenity::UserId, Arc<tokio::sync::Mutex<()>>>>,
    // Parsed once at boot. Re-reading these per event means a config typo takes
    // down a handler at some random future moment instead of failing the deploy.
    pub guild_id: serenity::GuildId,
    pub honeypot_channel_id: serenity::ChannelId,
    pub leetcode_channel_id: serenity::ChannelId,
    pub verified_role_id: serenity::RoleId,
    pub logs_channel_id: serenity::ChannelId,
    // Optional: only /weather uses it. Read once here so no handler re-reads the
    // process environment; `None` (missing or blank) simply disables /weather
    // rather than stopping the bot from booting.
    pub weather_token: Option<String>,
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

        // Non-fatal: the bot boots without a weather key; a missing or blank
        // value becomes None and disables /weather rather than failing.
        let weather_token = std::env::var("WEATHER_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty());

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
            verify_attempts: Mutex::new(HashMap::new()),
            verify_locks: Mutex::new(HashMap::new()),
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
    match error {
        poise::FrameworkError::Command { ctx, error, .. } => {
            // Print a redacted line ourselves and reply with a fixed generic ephemeral.
            // We deliberately do NOT forward this to `poise::builtins::on_error`: its
            // `Command` arm does a non-ephemeral `ctx.say(raw_error)`, which would leak
            // Supabase/DB error text (including student ids) into the channel (SEC-19).
            eprintln!(
                "[on_error] command '{}' failed: {}",
                ctx.command().name,
                redact_digits(&error.to_string())
            );
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
        other => {
            if let Err(e) = poise::builtins::on_error(other).await {
                eprintln!("[on_error] failed while handling a framework error: {e}");
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialise logging first, before anything can log. Default to `info`:
    // supabase-lib-rs logs generated query URLs (containing student IDs) and the
    // service-account email at `debug`, so RUST_LOG must never be set to debug or
    // trace on the VPS. See OPS-04 and the README.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

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
                commands::mods_only::unlink(),
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
