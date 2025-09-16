use eyre::Context;
use ical_merger::lib::{config::Config, error::Result, server::start_server};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().wrap_err("cannot load .env file")?;
    let config = envy::from_env::<Config>()?;

    start_server(config).await
}
