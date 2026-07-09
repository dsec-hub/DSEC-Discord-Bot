use crate::{ApplicationContext, Error};
use poise::CreateReply;
use serde::{Deserialize, Serialize};
use serenity::builder::CreateEmbed;
use supabase::Database;

#[derive(Deserialize, Serialize, Debug)]
pub struct MemberRow {
    pub full_name: String,
    pub student_id: String,
    pub campus: String,
    pub membership_status: String,
    pub end_date: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct DiscordLinkRow {
    student_id: String,
}

async fn member_data(database: &Database, user_id: &String) -> Result<Option<MemberRow>, Error> {
    let rows: Vec<MemberRow> = database
        .from("active_members")
        .select("student_id, full_name, campus, membership_status, end_date")
        .eq("student_id", user_id)
        .execute()
        .await?;
    let result = rows.into_iter().next();
    Ok(result)
}

/// Look up the student id linked to a given discord user, if any.
async fn recorded_member(database: &Database, user_id: &String) -> Result<Option<String>, Error> {
    let rows: Vec<DiscordLinkRow> = database
        .from("dsec_discord_members")
        .select("student_id")
        .eq("discord_id", user_id)
        .execute()
        .await?;
    Ok(rows.into_iter().next().map(|row| row.student_id))
}

/// Retrieve DSEC member information
#[poise::command(slash_command)]
pub async fn member_info(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let user = ctx.author();
    let database = ctx.data.state.supabase.database().clone();

    // check if discord ID is in table
    let linked_student_id = recorded_member(&database, &user.id.to_string()).await?;

    // if not present
    let Some(student_id) = linked_student_id else {
        ctx.send(
            CreateReply::default()
                .embed(
                    CreateEmbed::new()
                        .title("Re-verification required")
                        .description("Verify your membership in <#1433595503190474854>"),
                )
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let user_data_option = member_data(&database, &student_id).await?;

    if user_data_option.is_none() {
        ctx.send(CreateReply::default().embed(
            CreateEmbed::new().title("Couldn't find info").description(
                "Your membership may have expired. Contact a club executive to be sure.",
            ),
        ))
        .await?;
    } else {
        let user_data = user_data_option.expect("Member not found");
        let member_name = user_data.full_name;
        let member_campus = user_data.campus;
        let membership_status = user_data.membership_status;
        let expiry_date = user_data.end_date;

        ctx.send(
            CreateReply::default()
                .embed(
                    CreateEmbed::new()
                        .title("DSEC Membership Info")
                        .description(format!(
                            "
                        **Name:** {member_name}
                        **Campus:** {member_campus}
                        **Membership Status:** {membership_status}
                        **Membership Expiry Date**: {expiry_date}
                        "
                        )),
                )
                .ephemeral(true),
        )
        .await?;
    }

    // let student_data = state
    //     .supabase
    //     .database()
    //     .from("active_members")
    //     .select("full_name, student_id")
    //     .eq("student_id", &student_id)
    //     .execute()
    //     .await?;

    Ok(())
}
