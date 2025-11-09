use crate::{Context, Error};
use poise::CreateReply;
use serenity::{all::CreateEmbed, json::Value};

async fn get_weather(location: String) -> Result<String, Error> {
    let weather_api_key = std::env::var("WEATHER_TOKEN").expect("missing WEATHER_TOKEN");

    let request_url = format!(
        "https://api.weatherapi.com/v1/current.json?key={key}&q={location}",
        key = weather_api_key,
        location = location
    );

    // retrieve weather data
    let response = reqwest::get(request_url).await?.text().await?;

    // process weather data

    Ok(response)
}

/// Shows weather information
#[poise::command(slash_command)]
pub async fn weather(
    ctx: Context<'_>,
    #[description = "Location (City or Country)"] location: String,
) -> Result<(), Error> {
    let weather_response = get_weather(location).await?;
    let value: Value = serde_json::from_str(&weather_response)?;

    let location_name = (&value["location"]["name"]).as_str().unwrap();
    let location_region = (&value["location"]["region"]).as_str().unwrap();
    let location_country = (&value["location"]["country"]).as_str().unwrap();

    let weather_condition = (&value["current"]["condition"]["text"]).as_str().unwrap();
    let weather_temp = &value["current"]["temp_c"];
    let weather_feels_like = &value["current"]["feelslike_c"];
    let weather_wind_kph = &value["current"]["wind_kph"];
    let weather_humidity = &value["current"]["humidity"];
    let weather_cloud = &value["current"]["cloud"];

    let weather_icon = (&value["current"]["condition"]["icon"]).as_str().unwrap();

    let embed = CreateEmbed::new()
        .field("Name", format!("{}", location_name), true)
        .field("Region", format!("{}", location_region), true)
        .field("Country", format!("{}", location_country), true)
        .field("Condition", format!("{}", weather_condition), true)
        .field("Temperature", format!("{} °C", weather_temp), true)
        .field("Feels like", format!("{} °C", weather_feels_like), true)
        .field("wind", format!("{} kph", weather_wind_kph), true)
        .field("humidity", format!("{}%", weather_humidity), true)
        .field("cloud", format!("{}%", weather_cloud), true)
        .thumbnail(format!("https:{}", weather_icon));

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
