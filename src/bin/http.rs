use ical_merger::lib::{config::Config, error::Result, server::start_server};

#[tokio::main]
async fn main() -> Result<()> {
    let config = envy::from_env::<Config>()?;

    config.validate()?;

    start_server(config).await
}
