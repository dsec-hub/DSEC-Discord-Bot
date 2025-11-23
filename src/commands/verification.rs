use crate::{Data, Error};
use poise::{CreateReply, Modal};
use serde::{Deserialize, Serialize};
use serenity::all::{
    ComponentInteractionCollector, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
    GuildId, RoleId,
};

type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;

#[derive(Deserialize, Serialize, Debug)]
struct StudentRow {
    full_name: String,
    student_id: String,
}

#[derive(Debug, Modal)]
#[name = "Club Verification"] // Struct name by default
struct VerificationModal {
    #[name = "Full Name"]
    #[placeholder = "John Doe"]
    #[max_length = 50]
    name: String,
    #[name = "Student ID"]
    #[placeholder = "s123456789"]
    student_id: String,
}

#[poise::command(slash_command)]
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

    while let Some(_) = ComponentInteractionCollector::new(ctx.serenity_context())
        .filter(move |mci| mci.data.custom_id == "verify")
        .await
    {
        verify_member(ctx).await?;
    }

    Ok(())
}

// flow:
// 1. check correct discord server -> layer 1
// 2. check if user has role -> layer 1
// 3. get user input
// 4. check cache for membership -> layer 2
// 5. check db for membership + update cache -> layer 3

// Student ID not found: - add student ID to negative cache (cooldown) if ID not present (pending)
// Student ID found:
// - check if name matches full name
// - if match, assign role
async fn verify_member(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    // check if user is in a Discord server, then check if user is in THE Discord server
    let guild_id = ctx.guild_id();

    if guild_id.is_none() {
        ctx.say("Error, not in server").await?;
        return Ok(());
    }

    let server = GuildId::new(735865924829315072);

    let in_correct_server = &guild_id.unwrap() == &server; // CHANGE

    if !in_correct_server {
        ctx.say("Wrong server error. If you're in the DSEC server, let DSEC admins know")
            .await?;
        return Ok(());
    }

    // role ID 1441965955822649344
    let user_id = ctx.author().id;

    let verified_role_id = RoleId::new(1441965955822649344); // hardcoded for now
    let discord_member = GuildId::member(ctx.guild_id().unwrap(), ctx, user_id).await?;
    let has_role = discord_member.roles.contains(&verified_role_id);

    if has_role {
        let already_verified_embed =
            CreateEmbed::new()
                .title("Already Verified ✅")
                .description(format!(
                    "You already have the <@&{}> role!",
                    verified_role_id
                ));

        ctx.send(
            CreateReply::default()
                .embed(already_verified_embed)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // get user input
    let modal_data = VerificationModal::execute(ctx)
        .await?
        .expect("Modal failed");

    // remove s if input student ID starts with it
    let input_student_id: &str = &modal_data.student_id.to_lowercase();
    let student_id = input_student_id
        .strip_prefix("s")
        .unwrap_or(input_student_id);

    let state = &ctx.data().state;

    let student_in_cache: bool = {
        let cache = state.student_cache.lock().expect("Failed to get cache");

        println!("{:?}", cache.get(student_id));

        match cache.get(student_id) {
            Some(cached_name) => cached_name == &modal_data.name.to_lowercase(),
            None => false,
        }
    };

    if student_in_cache {
        discord_member.add_role(ctx, verified_role_id).await?;

        let verified_cache_embed = CreateEmbed::new()
            .title("Verified ✅")
            .description(format!(
                "You have been assigned the <@&{}> role!",
                verified_role_id
            ))
            .footer(CreateEmbedFooter::new("⚡ via cache"));

        ctx.send(
            CreateReply::default()
                .embed(verified_cache_embed)
                .ephemeral(true),
        )
        .await?;

        return Ok(());
    }

    // fetch from DB
    let student_data: Vec<StudentRow> = state
        .supabase
        .database()
        .from("active_members")
        .select("full_name, student_id")
        .eq("student_id", &student_id)
        .execute()
        .await?;

    let result = student_data.iter().next();

    // Student ID not found
    if result.is_none() {
        // TODO: add user to "don't use this command for 5 minutes"

        let id_not_found_embed = CreateEmbed::new()
        .title("Student ID not found!")
        .description("Your student ID is not found. It takes up to **a week** for your membership to be updated in the database since sign up. Try again later.");

        ctx.send(
            CreateReply::default()
                .embed(id_not_found_embed)
                .ephemeral(true),
        )
        .await?;

        return Ok(());
    }

    // get name from result
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

        let verified_embed = CreateEmbed::new().title("Verified ✅").description(format!(
            "You have been assigned the <@&{}> role!",
            verified_role_id
        ));

        ctx.send(CreateReply::default().embed(verified_embed))
            .await?;
    } else {
        let name_mismatched_embed = CreateEmbed::new()
            .title("Name mismatch ❌")
            .description("Your student ID is present, however the name does not match. Try again.");

        ctx.send(
            CreateReply::default()
                .embed(name_mismatched_embed)
                .ephemeral(true),
        )
        .await?;
    }

    Ok(())
}
