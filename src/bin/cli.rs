use ical_merger::lib::{
    calendar::{hide_details, merge_all_overlapping_events, urls_to_merged_calendar},
    config::Config,
    error::Result,
};

#[tokio::main]
async fn main() -> Result<()> {
    let config = envy::from_env::<Config>()?;

    config.validate()?;

    let mut calendar = urls_to_merged_calendar(config.urls, &config.tz_offsets).await?;

    if config.hide_details {
        calendar = hide_details(calendar);
        merge_all_overlapping_events(&mut calendar)
    }

    println!("{calendar}");

    Ok(())
}
