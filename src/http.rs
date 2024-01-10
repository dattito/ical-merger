use axum::extract::State;
use axum::routing::get;
use axum::Router;
use cached::proc_macro::once;

use crate::config::Config;
use crate::{
    error::{Error, Result},
    merger::urls_to_merged_calendar,
};

pub async fn start_server(config: Config) -> Result<()> {
    let app = Router::new()
        .route("/", get(handler))
        .with_state(config.clone());

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", &config.host, &config.port)).await?;

    println!("Started listening on {}:{}", &config.host, &config.port);

    axum::serve(listener, app).await.map_err(Error::IO)
}

#[once(time=900, result=true, sync_writes=true)]
async fn handler(State(config): State<Config>) -> Result<String> {
      // cached_calendar(config.urls).await
    Ok(urls_to_merged_calendar(config.urls).await?.to_string())
}
