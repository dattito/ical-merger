use axum::extract::State;
use axum::routing::get;
use axum::Router;
use cached::proc_macro::once;
use tokio::{signal, time::Duration};

use crate::lib::{
    calendar::{filter_future_days, hide_details, urls_to_merged_calendar},
    config::Config,
    error::{Error, Result},
};

pub async fn start_server(config: Config) -> Result<()> {
    let app = Router::new()
        .route("/", get(handler))
        .with_state(config.clone());

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", &config.host, &config.port)).await?;

    println!("Started listening on {}:{}", &config.host, &config.port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(Error::IO)
}

#[once(time = 900, result = true, sync_writes = true)]
async fn handler(State(config): State<Config>) -> Result<String> {
    // cached_calendar(config.urls).await
    let mut c = urls_to_merged_calendar(config.urls, &config.tz_offsets).await?;

    if let Some(days_limit) = config.future_days_limit {
        c = filter_future_days(c, days_limit);
    }

    match config.hide_details {
        true => Ok(hide_details(c).to_string()),
        false => Ok(c.to_string()),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
