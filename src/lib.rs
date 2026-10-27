#![forbid(unsafe_code)]

pub mod auth;
pub mod books;
pub mod index;
pub mod media;
pub mod metadata;
pub mod playback;
mod sessions;
pub mod telemetry;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::{Mutex, Notify};
use tower_http::compression::CompressionLayer;
use tower_sessions::{Expiry, Session, SessionManagerLayer, cookie::SameSite};

#[derive(Clone)]
pub struct Config {
    pub library_directory: PathBuf,
    pub data_directory: PathBuf,
    pub public_origin: String,
    pub secure_cookies: bool,
    pub scan_interval_seconds: u64,
    pub inspector_executable: PathBuf,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let public_url =
            std::env::var("KAPITEL_PUBLIC_URL").unwrap_or_else(|_| "http://localhost:8080".into());
        let allow_lan_http = std::env::var("KAPITEL_ALLOW_LAN_HTTP")
            .unwrap_or_else(|_| "false".into())
            .parse::<bool>()?;
        let (public_origin, secure_cookies) = validate_public_origin(&public_url, allow_lan_http)?;
        let scan_interval_seconds = std::env::var("KAPITEL_SCAN_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "30".into())
            .parse()?;
        anyhow::ensure!(
            scan_interval_seconds >= 5,
            "Scan interval must be at least 5 seconds"
        );
        Ok(Self {
            library_directory: std::env::var_os("KAPITEL_LIBRARY_DIRECTORY")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/library".into()),
            data_directory: std::env::var_os("KAPITEL_DATA_DIRECTORY")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/data".into()),
            public_origin,
            secure_cookies,
            scan_interval_seconds,
            inspector_executable: std::env::current_exe()?,
        })
    }
}

fn validate_public_origin(
    public_url: &str,
    allow_lan_http: bool,
) -> anyhow::Result<(String, bool)> {
    let url = url::Url::parse(public_url)?;
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "KAPITEL_PUBLIC_URL must be an origin, without credentials, path, query or fragment"
    );
    let secure_cookies = url.scheme() == "https";
    let localhost = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let private_address = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_private(),
        Some(url::Host::Ipv6(address)) => address.is_unique_local(),
        _ => false,
    };
    anyhow::ensure!(
        secure_cookies
            || (url.scheme() == "http" && (localhost || (allow_lan_http && private_address))),
        "Use HTTPS, or explicitly enable KAPITEL_ALLOW_LAN_HTTP for testing on a private IP address"
    );
    Ok((url.origin().ascii_serialization(), secure_cookies))
}

#[cfg(test)]
mod config_tests {
    use super::validate_public_origin;

    #[test]
    fn lan_http_requires_explicit_opt_in_and_a_private_ip() {
        assert!(validate_public_origin("http://192.168.1.20:8080", false).is_err());
        for origin in [
            "http://192.168.1.20:8080",
            "http://10.1.2.3:8080",
            "http://172.16.0.5:8080",
            "http://[fd00::1]:8080",
        ] {
            assert_eq!(
                validate_public_origin(origin, true).unwrap(),
                (origin.into(), false)
            );
        }
        for origin in [
            "http://8.8.8.8:8080",
            "http://books.example.org",
            "http://0.0.0.0:8080",
            "http://169.254.169.254",
            "http://192.168.1.20/private",
            "http://user:password@192.168.1.20",
            "ftp://192.168.1.20",
        ] {
            assert!(validate_public_origin(origin, true).is_err(), "{origin}");
        }
        assert!(
            !validate_public_origin("http://localhost:8080", false)
                .unwrap()
                .1
        );
        assert!(
            validate_public_origin("https://books.example.org", false)
                .unwrap()
                .1
        );
    }
}

#[derive(Clone)]
pub struct AppState {
    pub telemetry: Option<opentelemetry_sdk::trace::SdkTracer>,
    pub database: SqlitePool,
    pub config: Config,
    pub scan_requested: Arc<Notify>,
    pub scan_status: Arc<Mutex<ScanStatus>>,
    pub login_attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    pub password_work: Arc<tokio::sync::Semaphore>,
    pub dummy_password_hash: Arc<String>,
}

#[derive(Default, Clone, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub last_completed_at: Option<i64>,
    pub message: String,
}

pub async fn initialize(config: Config) -> anyhow::Result<AppState> {
    tokio::fs::create_dir_all(&config.data_directory).await?;
    let options = SqliteConnectOptions::new()
        .filename(config.data_directory.join("kapitel.sqlite"))
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(std::time::Duration::from_secs(5));
    let database = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    let has_schema: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
    )
    .fetch_one(&database)
    .await?;
    if has_schema {
        let version: i64 = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_one(&database)
            .await?;
        anyhow::ensure!(
            version == 1,
            "Unsupported database schema; use the matching Kapitel version"
        );
    } else {
        let mut transaction = database.begin().await?;
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
    }
    let dummy_password_hash = auth::hash_password(&uuid::Uuid::new_v4().to_string())?;
    Ok(AppState {
        telemetry: None,
        database,
        config,
        scan_requested: Arc::new(Notify::new()),
        scan_status: Arc::default(),
        login_attempts: Arc::default(),
        password_work: Arc::new(tokio::sync::Semaphore::new(2)),
        dummy_password_hash: Arc::new(dummy_password_hash),
    })
}

pub async fn router(state: AppState) -> anyhow::Result<Router> {
    let session_store = sessions::SqliteSessionStore(state.database.clone());
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name("kapitel_session")
        .with_secure(state.config.secure_cookies)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(30)));
    Ok(Router::new()
        .route(
            "/",
            get(|| async {
                asset(
                    "text/html; charset=utf-8",
                    include_bytes!("../web/index.html"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                asset(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../web/app.js"),
                )
            }),
        )
        .route(
            "/player.js",
            get(|| async {
                asset(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../web/player.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                asset(
                    "text/css; charset=utf-8",
                    include_bytes!("../web/style.css"),
                )
            }),
        )
        .route("/health", get(|| async { "ok" }))
        .route(
            "/api/session",
            get(auth::current)
                .post(auth::login)
                .delete(auth::logout)
                .patch(auth::preferences),
        )
        .route("/api/books", get(books::list))
        .route("/api/books/{book_id}", get(books::detail))
        .route("/api/books/{book_id}/cover", get(books::cover))
        .route(
            "/api/media/{media_file_id}",
            get(media::serve).head(media::serve),
        )
        .route("/api/playback-sessions", post(playback::start))
        .route(
            "/api/playback-sessions/{playback_session_id}/position",
            axum::routing::put(playback::save),
        )
        .route(
            "/api/admin/profiles",
            get(auth::profiles).post(auth::create_profile),
        )
        .route(
            "/api/admin/profiles/{profile_id}",
            axum::routing::patch(auth::update_profile),
        )
        .route(
            "/api/admin/books/{book_id}",
            axum::routing::patch(books::update),
        )
        .route(
            "/api/admin/scan",
            get(index::status).post(index::request_scan),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .layer(session_layer)
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            telemetry::request,
        ))
        .with_state(state))
}

fn asset(content_type: &'static str, content: &'static [u8]) -> Response {
    (
        [
            ("content-type", content_type),
            ("cache-control", "no-cache"),
        ],
        content,
    )
        .into_response()
}

async fn security(
    State(state): State<AppState>,
    session: Session,
    request: Request,
    next: Next,
) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        if request
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            != Some(&state.config.public_origin)
        {
            return AppError::forbidden("The request origin is not allowed").into_response();
        }
        if request.uri().path() != "/api/session" || request.method() != Method::POST {
            let csrf_token = session.get::<String>("csrf_token").await.ok().flatten();
            if csrf_token.is_none()
                || request
                    .headers()
                    .get("x-csrf-token")
                    .and_then(|v| v.to_str().ok())
                    != csrf_token.as_deref()
            {
                return AppError::forbidden("Refresh the page before trying again").into_response();
            }
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; media-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers
        .entry("cache-control")
        .or_insert(HeaderValue::from_static("private, no-store"));
    if state.config.secure_cookies {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}

#[derive(Debug)]
pub struct AppError(pub StatusCode, pub &'static str, pub String);
impl AppError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, "invalid_request", message.into())
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self(StatusCode::FORBIDDEN, "forbidden", message.into())
    }
    pub fn missing() -> Self {
        Self(
            StatusCode::NOT_FOUND,
            "not_found",
            "This item is unavailable".into(),
        )
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, "conflict", message.into())
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(serde_json::json!({"error": {"code": self.1, "message": self.2}})),
        )
            .into_response()
    }
}
impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(error = %error, "database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "The operation could not be saved. Please try again.".into(),
        )
    }
}
impl From<tower_sessions::session::Error> for AppError {
    fn from(_: tower_sessions::session::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_error",
            "The session could not be saved".into(),
        )
    }
}
pub type Result<T> = std::result::Result<T, AppError>;
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
