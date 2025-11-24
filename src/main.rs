use dotenv::dotenv;
use poise::serenity_prelude as serenity;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use supabase::prelude::*;
mod commands;

pub struct Data {
    pub state: AppState,
}

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

#[derive(Clone)]
pub struct AppState {
    pub supabase: Arc<Client>,
    pub student_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        dotenv().ok();
        let supabase_url = std::env::var("SUPABASE_URL").expect("missing SUPABASE_URL");
        let supabase_key = std::env::var("SUPABASE_KEY").expect("missing SUPABASE_KEY");
        let client = Client::new(&supabase_url, &supabase_key)?;

        Ok(Self {
            supabase: Arc::new(client),
            student_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }
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
            event_handler: |_ctx, _event, _framework, _data| {
                Box::pin(async move {
                    // println!("Got an event in event handler: {:?}", event);

                    Ok(())
                })
            },

            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                println!("Logged in as {}", _ready.user.name);
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
