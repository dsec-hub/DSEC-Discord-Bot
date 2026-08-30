use crate::{Context, Error};
use poise::CreateReply;
use serenity::{all::CreateEmbed, json::Value};

/// Fetch the raw weather response for a location. Returns the HTTP status
/// alongside the parsed JSON body so the caller can tell a success payload from
/// an API error payload (e.g. an unknown location) without unwrapping anything
/// on external JSON. A body that is not valid JSON parses to `Value::Null`.
async fn get_weather(
    location: &str,
    weather_api_key: &str,
) -> Result<(reqwest::StatusCode, Value), Error> {
    let request_url = format!(
        "https://api.weatherapi.com/v1/current.json?key={key}&q={location}",
        key = weather_api_key,
        location = location
    );

    let response = reqwest::get(request_url).await?;
    let status = response.status();
    let body = response.text().await?;
    let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    Ok((status, value))
}

/// Shows weather information
#[poise::command(slash_command)]
pub async fn weather(
    ctx: Context<'_>,
    #[description = "Location (City or Country)"] location: String,
) -> Result<(), Error> {
    // Weather is optional. Without a configured token the command is disabled
    // rather than panicking on a missing key.
    let Some(weather_api_key) = ctx.data().state.weather_token.as_deref() else {
        ctx.send(
            CreateReply::default()
                .content("The weather command is not configured on this bot.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let (status, value) = get_weather(&location, weather_api_key).await?;

    // weatherapi.com returns a JSON error body (unknown location, bad key, quota)
    // with a non-2xx status. Surface a friendly message instead of unwrapping
    // fields that are not present in an error payload.
    if !status.is_success() {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Could not fetch the weather for that location.");
        ctx.send(
            CreateReply::default()
                .content(format!("Weather lookup failed: {message}"))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // Read every field defensively — external JSON is never unwrapped.
    let text = |ptr: &str| -> String {
        value
            .pointer(ptr)
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string()
    };
    let number = |ptr: &str| -> String {
        match value.pointer(ptr) {
            Some(v) if !v.is_null() => v.to_string(),
            _ => "?".to_string(),
        }
    };

    let mut embed = CreateEmbed::new()
        .field("Name", text("/location/name"), true)
        .field("Region", text("/location/region"), true)
        .field("Country", text("/location/country"), true)
        .field("Condition", text("/current/condition/text"), true)
        .field(
            "Temperature",
            format!("{} °C", number("/current/temp_c")),
            true,
        )
        .field(
            "Feels like",
            format!("{} °C", number("/current/feelslike_c")),
            true,
        )
        .field("Wind", format!("{} kph", number("/current/wind_kph")), true)
        .field(
            "Humidity",
            format!("{}%", number("/current/humidity")),
            true,
        )
        .field("Cloud", format!("{}%", number("/current/cloud")), true);

    // Only set the thumbnail when the API actually returned an icon URL — a
    // placeholder would produce an invalid "https:Unknown" URL that Discord rejects.
    if let Some(icon) = value
        .pointer("/current/condition/icon")
        .and_then(Value::as_str)
    {
        embed = embed.thumbnail(format!("https:{icon}"));
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
