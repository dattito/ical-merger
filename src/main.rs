use crate::{error::Result, http::start_server};

mod config;
mod error;
mod http;
mod merger;

#[tokio::main]
async fn main() -> Result<()> {
    let config = envy::from_env::<config::Config>()?;

    start_server(config).await
}
