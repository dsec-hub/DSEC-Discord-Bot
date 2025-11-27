use crate::Error;
use poise::serenity_prelude as serenity;

#[derive(Debug, poise::Modal)]
#[allow(dead_code)] // fields only used for Debug print
struct MyModal {
    first_input: String,
    second_input: Option<String>,
}

pub async fn on_interaction_create(
    _ctx: &serenity::Context,
    interaction: &serenity::Interaction,
) -> Result<(), Error> {
    let some_message_component = interaction.as_message_component();

    if !some_message_component.is_none() {
        let component = some_message_component.unwrap();

        if component.data.custom_id == "verify" {
            println!("Component data: {:?}", component);
        }
    }

    Ok(())
}
