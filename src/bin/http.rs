use ical_merger::lib::{config::Config, error::Result, server::start_server};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing();
    let config = envy::from_env::<Config>()?;

    config.validate()?;

    start_server(config).await
}

fn setup_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "ical_merger=debug,tower_http=debug,axum::rejection=trace".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}
