use dotenv::dotenv;
use poise::serenity_prelude as serenity;
use std::{collections::HashMap, sync::Mutex};
use supabase::Client;
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
}

impl AppState {
    pub fn new() -> supabase::Result<Self> {
        dotenv().ok();
        let supabase_url = std::env::var("SUPABASE_URL").expect("missing SUPABASE_URL");
        let supabase_key = std::env::var("SUPABASE_KEY").expect("missing SUPABASE_KEY");
        let client = Client::new(&supabase_url, &supabase_key)?;

        Ok(Self {
            supabase: client,
            student_cache: Mutex::new(HashMap::new()),
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
            events::interaction_create::on_interaction_create(ctx, interaction, &data).await?;
        }
        _ => {}
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    dotenv().ok(); // load env

    let app_state = AppState::new().expect("Failed to initialize AppState");

    // -- discord bot start --
    let token = std::env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents = serenity::GatewayIntents::non_privileged();

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
            ],
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },

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
        .expect("Client failed to start 2");

    // -- discord bot end --
}
