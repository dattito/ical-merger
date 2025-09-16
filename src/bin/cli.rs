use eyre::Context;
use ical_merger::lib::{
    calendar::{filter_future_days, hide_details, urls_to_merged_calendar},
    config::Config,
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenvy::dotenv().wrap_err("cannot load .env file")?;
    let config = envy::from_env::<Config>().wrap_err("cannot get config from env")?;

    let mut calendar = urls_to_merged_calendar(config.urls, &config.tz_offsets).await?;

    if let Some(days_limit) = config.future_days_limit {
        calendar = filter_future_days(calendar, days_limit);
    }

    if config.hide_details {
        calendar = hide_details(calendar);
    }

    println!("{calendar}");

    Ok(())
}
