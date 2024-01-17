use ical_merger::lib::{
    calendar::{hide_details, urls_to_merged_calendar},
    config::Config,
    error::Result,
};

#[tokio::main]
async fn main() -> Result<()> {
    let config = envy::from_env::<Config>()?;

    let mut calendar = urls_to_merged_calendar(config.urls, &config.tz_offsets).await?;

    if config.hide_details {
        calendar = hide_details(calendar);
    }

    println!("{calendar}");

    Ok(())
}
