use ical_merger::{config::Config, server::start_server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ical_merger=info")),
        )
        .init();

    let config = Config::load()?;
    start_server(config).await?;
    Ok(())
}
