use crate::{Context, Error};
use poise::CreateReply;
use serenity::all::{ChannelType, Colour, CreateEmbed, CreateEmbedFooter, GuildId, User};
use std::{collections::HashMap, time::Instant};

/// Show this help menu
#[poise::command(track_edits, slash_command)]
pub async fn help(
    ctx: Context<'_>,
    #[description = "Specific command to show help about"]
    #[autocomplete = "poise::builtins::autocomplete_command"]
    command: Option<String>,
) -> Result<(), Error> {
    poise::builtins::help(
        ctx,
        command.as_deref(),
        poise::builtins::HelpConfiguration {
            extra_text_at_bottom: "made using Serenity Poise framework in Rust and with ❤️",
            ..Default::default()
        },
    )
    .await?;
    Ok(())
}

/// Ping the bot
#[poise::command(slash_command)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    // send embed -> take how long it took -> edit message about how long it took
    let start = Instant::now();

    let embed_color = Colour::DARK_GREY;

    let start_embed_msg = CreateEmbed::new().title("Pinging...").color(embed_color);
    let first_reply: CreateReply = CreateReply::default().embed(start_embed_msg);

    let msg = ctx.send(first_reply).await?;

    // record ping
    let elapsed_ms = start.elapsed().as_millis();

    // create the 2nd embed
    let result_embed_msg = CreateEmbed::new()
        .title("Pong!")
        .description(format!("{} ms", elapsed_ms))
        .color(embed_color);

    msg.edit(ctx, CreateReply::default().embed(result_embed_msg))
        .await?;

    Ok(())
}

/// Display user's information
#[poise::command(slash_command)]
pub async fn userinfo(
    ctx: Context<'_>,
    #[description = "Specific user to show information about"]
    #[autocomplete = "poise::builtins::autocomplete_command"]
    user: Option<User>,
) -> Result<(), Error> {
    let discord_user: User = user.unwrap_or(ctx.author().to_owned());
    let discord_member = GuildId::member(ctx.guild_id().unwrap(), ctx, discord_user.id).await?;

    // color
    let embed_color = discord_user.accent_colour.unwrap_or(Colour::DARK_GREY);

    // member dates
    let member_created_at = discord_user.created_at().format("%d/%m/%Y %I:%M %p");
    let member_joined_at = discord_member
        .joined_at
        .unwrap()
        .format("%d/%m/%Y %I:%M %p");

    // user information
    let user_id = &discord_user.id;
    let user_avatar_url = discord_user
        .avatar_url()
        .unwrap_or(discord_user.default_avatar_url());
    let username = &discord_user.name;

    // member information
    let nickname = &discord_member.nick.unwrap_or(username.to_owned());
    let role_ids = discord_member.roles;
    let role_mentions: Vec<String> = role_ids
        .into_iter()
        .map(|role_id| format!("<@&{}>", role_id))
        .collect();

    // embed
    let result_embed_msg = CreateEmbed::new()
        .thumbnail(user_avatar_url)
        .color(embed_color)
        .title("User Info")
        .field("Display Name", nickname, true)
        .field("Username", username, true)
        .field("Created At", format!("{}", member_created_at), false)
        .field("Joined At", format!("{}", member_joined_at), false)
        .field("Roles", role_mentions.join(" "), false)
        .footer(CreateEmbedFooter::new(format!("ID: {}", user_id)));

    ctx.send(CreateReply::default().embed(result_embed_msg))
        .await?;

    Ok(())
}

/// Display server's information
#[poise::command(slash_command)]
pub async fn serverinfo(ctx: Context<'_>) -> Result<(), Error> {
    let embed_color = Colour::DARK_GREY;

    // partial guild allows to access server information (excluding members information) without cache
    let partial_guild = ctx.partial_guild().await.unwrap();

    // getting accurate member count from cache ref
    let member_count = {
        let guild = ctx.guild();
        guild.as_deref().unwrap().member_count
    };

    // count different types of channels
    let channels_hashmap = partial_guild.clone().channels(ctx).await?;
    let guild_channels = channels_hashmap.into_values();

    let mut counts: HashMap<ChannelType, u32> = HashMap::new();

    for channel in guild_channels {
        let kind = match channel.kind {
            ChannelType::Category => ChannelType::Category,
            ChannelType::Text => ChannelType::Text,
            ChannelType::Voice => ChannelType::Voice,
            _ => ChannelType::default(),
        };

        *counts.entry(kind).or_insert(0) += 1;
    }

    let category_channel_count = counts.get(&ChannelType::Category).unwrap_or(&0);
    let text_channel_count = counts.get(&ChannelType::Text).unwrap_or(&0);
    let voice_channel_count = counts.get(&ChannelType::Voice).unwrap_or(&0);

    // server information
    let server_id = &partial_guild.id;
    let server_name = &partial_guild.name;
    let server_icon = &partial_guild.icon_url().unwrap_or_default();
    let owner_id = &partial_guild.owner_id;

    // server description, if empty N/A
    let server_description_option = &partial_guild.description;
    let server_description = server_description_option.as_deref().unwrap_or("N/A");

    // rules channel, if empty N/A
    let rules_channel = if (&partial_guild.rules_channel_id).is_none() {
        "N/A"
    } else {
        &format!("<#{}>", &partial_guild.rules_channel_id.unwrap())
    };

    let embed_footer = CreateEmbedFooter::new(format!("ID: {}", server_id));

    let result_embed_msg = CreateEmbed::new()
        .thumbnail(server_icon)
        .title(server_name)
        .field("Owner", format!("<@{}>", owner_id), true)
        .field("Rules", rules_channel, true)
        .field("Members", format!("{}", member_count), true)
        .field(
            "Category Channels",
            format!("{}", category_channel_count),
            true,
        )
        .field("Text Channels", format!("{}", text_channel_count), true)
        .field("Voice Channels", format!("{}", voice_channel_count), true)
        .field("Description", server_description, false)
        .footer(embed_footer)
        .color(embed_color);

    ctx.send(CreateReply::default().embed(result_embed_msg))
        .await?;

    Ok(())
}
