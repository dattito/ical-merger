use axum::extract::State;
use axum::routing::get;
use axum::Router;
use cached::proc_macro::once;
use tracing::info;

use crate::lib::{
    calendar::{hide_details, merge_all_overlapping_events, urls_to_merged_calendar},
    config::Config,
    error::{Error, Result},
};

#[tracing::instrument(skip_all)]
pub async fn start_server(config: Config) -> Result<()> {
    let app = Router::new()
        .route("/", get(handler))
        .with_state(config.clone());

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", &config.host, &config.port)).await?;

    info!("Started listening on {}:{}", &config.host, &config.port);

    axum::serve(listener, app).await.map_err(Error::IO)
}

#[tracing::instrument(skip_all)]
async fn handler(State(config): State<Config>) -> Result<String> {
    info!("Processing request");

    get_cached_calendar(config).await
}

#[tracing::instrument(skip_all)]
#[once(time = 900, result = true, sync_writes = true)]
async fn get_cached_calendar(config: Config) -> Result<String> {
    info!("Get fresh calendar data");
    let mut c = urls_to_merged_calendar(config.urls, &config.tz_offsets).await?;

    if config.hide_details {
        c = hide_details(c);
    }

    if config.merge_overlapping_events {
        merge_all_overlapping_events(&mut c)
    }

    Ok(c.to_string())
}
