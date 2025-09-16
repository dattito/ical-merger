use ical_merger::lib::{config::Config, error::Result, server::start_server};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = envy::from_env::<Config>()?;

    start_server(config).await
}
