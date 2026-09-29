use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use chrono::{DateTime, Days, NaiveTime, TimeZone, Utc};
use futures::future::join_all;
use tokio::{signal, sync::Mutex};
use tracing::{info, warn};

use crate::{
    calendar::{self, ParsedSource, Window},
    config::{Config, SourceConfig},
};

const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    config: Arc<Config>,
    client: reqwest::Client,
    sources: Vec<SourceRuntime>,
}

#[derive(Clone)]
struct SourceRuntime {
    config: SourceConfig,
    cache: Arc<Mutex<Option<CachedSource>>>,
}

#[derive(Clone)]
struct CachedSource {
    fetched_at: Instant,
    calendar: Arc<ParsedSource>,
}

#[derive(Debug)]
enum SourceError {
    Network,
    HttpStatus(StatusCode),
    TooLarge,
    InvalidUtf8,
    InvalidCalendar,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self, ServerError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("ical-merger/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ServerError::Client)?;
        let sources = config
            .sources
            .iter()
            .cloned()
            .map(|config| SourceRuntime {
                config,
                cache: Arc::new(Mutex::new(None)),
            })
            .collect();
        Ok(Self {
            config: Arc::new(config),
            client,
            sources,
        })
    }

    async fn load_source(&self, source: &SourceRuntime) -> Result<Arc<ParsedSource>, SourceError> {
        let mut cache = source.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.fetched_at.elapsed() < self.config.refresh_interval {
                return Ok(Arc::clone(&cached.calendar));
            }
        }

        let body = fetch_source(&self.client, &source.config.url).await?;
        let parsed = ParsedSource::parse(&body, &source.config, self.config.default_timezone)
            .map_err(|_| SourceError::InvalidCalendar)?;
        let parsed = Arc::new(parsed);
        *cache = Some(CachedSource {
            fetched_at: Instant::now(),
            calendar: Arc::clone(&parsed),
        });
        Ok(parsed)
    }
}

async fn fetch_source(client: &reqwest::Client, url: &url::Url) -> Result<String, SourceError> {
    let mut response = client
        .get(url.clone())
        .header(
            header::ACCEPT,
            "text/calendar, application/calendar, text/plain",
        )
        .send()
        .await
        .map_err(|_| SourceError::Network)?;
    if !response.status().is_success() {
        return Err(SourceError::HttpStatus(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SOURCE_BYTES as u64)
    {
        return Err(SourceError::TooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| SourceError::Network)? {
        if body.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err(SourceError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| SourceError::InvalidUtf8)
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("could not configure HTTP client")]
    Client,
    #[error("could not bind HTTP listener: {0}")]
    Bind(#[from] std::io::Error),
    #[error("HTTP server failed: {0}")]
    Serve(std::io::Error),
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(calendar_handler))
        .route("/healthz", get(health_handler))
        .with_state(state)
}

pub async fn start_server(config: Config) -> Result<(), ServerError> {
    let listen = config.listen;
    let app = app(AppState::new(config)?);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    info!(listen = %listen, "calendar feed server started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(ServerError::Serve)
}

async fn calendar_handler(State(state): State<AppState>) -> Response<Body> {
    let now = Utc::now();
    let window = match build_window(
        now,
        state.config.default_timezone,
        state.config.horizon_days,
    ) {
        Ok(window) => window,
        Err(()) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let results = join_all(state.sources.iter().enumerate().map(|(index, source)| {
        let state = state.clone();
        async move { (index, state.load_source(source).await) }
    }))
    .await;

    let mut available_sources = Vec::new();
    let mut failed_sources = 0;
    for (index, result) in results {
        match result {
            Ok(calendar) => available_sources.push((index, (*calendar).clone())),
            Err(error) => {
                failed_sources += 1;
                warn!(
                    source = index,
                    issue = source_error_name(&error),
                    http_status = source_error_status(&error),
                    "calendar source unavailable"
                );
            }
        }
    }
    if available_sources.is_empty() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let rendered = match calendar::render(&available_sources, window, state.config.output, now) {
        Ok(rendered) => rendered,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let is_partial = failed_sources > 0 || rendered.skipped_events > 0;
    if is_partial {
        warn!(
            skipped_sources = failed_sources,
            skipped_events = rendered.skipped_events,
            "serving a partial calendar feed"
        );
    }

    let mut response = Response::new(Body::from(rendered.body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&format!(
            "public, max-age={}",
            state.config.refresh_interval.as_secs().min(900)
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("public, max-age=60")),
    );
    response.headers_mut().insert(
        "x-ical-merger-partial",
        HeaderValue::from_static(if is_partial { "true" } else { "false" }),
    );
    response.headers_mut().insert(
        "x-ical-merger-skipped-sources",
        HeaderValue::from_str(&failed_sources.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    response.headers_mut().insert(
        "x-ical-merger-skipped-events",
        HeaderValue::from_str(&rendered.skipped_events.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    response
}

async fn health_handler() -> &'static str {
    "ok\n"
}

fn source_error_name(error: &SourceError) -> &'static str {
    match error {
        SourceError::Network => "network-error",
        SourceError::HttpStatus(_) => "http-error",
        SourceError::TooLarge => "response-too-large",
        SourceError::InvalidUtf8 => "invalid-utf8",
        SourceError::InvalidCalendar => "invalid-calendar",
    }
}

fn source_error_status(error: &SourceError) -> Option<u16> {
    match error {
        SourceError::HttpStatus(status) => Some(status.as_u16()),
        _ => None,
    }
}

fn build_window(now: DateTime<Utc>, timezone: chrono_tz::Tz, days: u32) -> Result<Window, ()> {
    let today = now.with_timezone(&timezone).date_naive();
    let start = local_midnight(today, timezone)?;
    let end_date = today
        .checked_add_days(Days::new(u64::from(days)))
        .ok_or(())?;
    let end = local_midnight(end_date, timezone)?;
    Ok(Window { start, end })
}

fn local_midnight(date: chrono::NaiveDate, timezone: chrono_tz::Tz) -> Result<DateTime<Utc>, ()> {
    let local = date.and_time(NaiveTime::MIN);
    match timezone.from_local_datetime(&local) {
        chrono::LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(first, second) => Ok(first.min(second).with_timezone(&Utc)),
        chrono::LocalResult::None => Err(()),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut terminate) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            terminate.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use super::*;

    const MIXED_CALENDAR: &str = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:valid\nDTSTART:20990101T100000Z\nDTEND:20990101T110000Z\nSUMMARY:Future\nEND:VEVENT\nBEGIN:VEVENT\nUID:invalid\nDTSTART;TZID=Unknown/Zone:20990101T100000\nDTEND;TZID=Unknown/Zone:20990101T110000\nEND:VEVENT\nEND:VCALENDAR\n";

    async fn mock_server() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let ok_requests = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/ok",
                get(move || {
                    let requests = Arc::clone(&ok_requests);
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        MIXED_CALENDAR
                    }
                }),
            )
            .route("/fail", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{address}/"), requests, task)
    }

    async fn flaky_server() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let state = Arc::clone(&requests);
        let app = Router::new().route(
            "/calendar",
            get(move || {
                let state = Arc::clone(&state);
                async move {
                    if state.fetch_add(1, Ordering::SeqCst) == 0 {
                        (StatusCode::OK, MIXED_CALENDAR).into_response()
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{address}/"), requests, task)
    }

    fn config(urls: &[String], refresh_seconds: u64) -> Config {
        let sources = urls
            .iter()
            .map(|url| format!("[[sources]]\nurl = {url:?}\n"))
            .collect::<String>();
        Config::parse(&format!(
            "default_timezone = \"UTC\"\nrefresh_seconds = {refresh_seconds}\n{sources}"
        ))
        .unwrap()
    }

    async fn get_feed(app: Router) -> Response<Body> {
        app.oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn partial_feed_reports_failures_and_caches_successful_sources() {
        let (base, requests, server) = mock_server().await;
        let urls = vec![
            format!("{base}ok"),
            format!("{base}fail?token=secret-source-token"),
        ];
        let state = AppState::new(config(&urls, 900)).unwrap();
        let app = app(state);

        let response = get_feed(app.clone()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ical-merger-partial"], "true");
        assert_eq!(response.headers()["x-ical-merger-skipped-sources"], "1");
        assert_eq!(response.headers()["x-ical-merger-skipped-events"], "1");
        assert!(response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/calendar"));
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("secret-source-token"));
        assert!(!body.contains(&urls[0]));
        assert!(mailrs_ical::parse::parse_calendar(&body).is_ok());

        let second = get_feed(app).await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn all_failed_sources_return_service_unavailable() {
        let (base, _, server) = mock_server().await;
        let state = AppState::new(config(&[format!("{base}fail")], 900)).unwrap();
        let response = get_feed(app(state)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        server.abort();
    }

    #[tokio::test]
    async fn successful_source_cache_refreshes_after_its_interval() {
        let (base, requests, server) = mock_server().await;
        let state = AppState::new(config(&[format!("{base}ok")], 1)).unwrap();
        let app = app(state);

        assert_eq!(get_feed(app.clone()).await.status(), StatusCode::OK);
        assert_eq!(get_feed(app.clone()).await.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(get_feed(app).await.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn expired_successful_cache_is_not_used_after_refresh_failure() {
        let (base, requests, server) = flaky_server().await;
        let state = AppState::new(config(&[format!("{base}calendar")], 1)).unwrap();
        let app = app(state);

        assert_eq!(get_feed(app.clone()).await.status(), StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(
            get_feed(app).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn health_check_does_not_fetch_calendar_sources() {
        let (base, requests, server) = mock_server().await;
        let state = AppState::new(config(&[format!("{base}ok")], 900)).unwrap();
        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        server.abort();
    }
}
