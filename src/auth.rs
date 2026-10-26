use crate::{AppError, AppState, Result, new_id};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::time::{Duration, Instant};
use tower_sessions::Session;

#[derive(Clone, Serialize, FromRow)]
pub struct Profile {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
    pub enabled: bool,
    pub playback_rate: f64,
    #[serde(skip)]
    pub auth_version: i64,
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|hash| hash.to_string())
        .map_err(|_| anyhow::anyhow!("Password hashing failed"))
}

fn validate_credentials(username: &str, password: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > 48
        || !username
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(AppError::bad(
            "Username must contain 1–48 letters, digits, dots, underscores or hyphens",
        ));
    }
    if password.chars().count() < 15 || password.len() > 512 {
        return Err(AppError::bad(
            "Use a password of at least 15 characters and at most 512 bytes",
        ));
    }
    Ok(())
}

pub async fn require_profile(state: &AppState, session: &Session) -> Result<Profile> {
    let unauthorized = || {
        AppError(
            StatusCode::UNAUTHORIZED,
            "sign_in_required",
            "Please sign in".into(),
        )
    };
    let profile_id = session
        .get::<String>("profile_id")
        .await?
        .ok_or_else(unauthorized)?;
    let auth_version = session
        .get::<i64>("auth_version")
        .await?
        .ok_or_else(unauthorized)?;
    let profile = sqlx::query_as::<_, Profile>(
        "SELECT * FROM profiles WHERE id=? AND enabled=1 AND auth_version=?",
    )
    .bind(profile_id)
    .bind(auth_version)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(unauthorized)?;
    Ok(profile)
}
pub async fn require_admin(state: &AppState, session: &Session) -> Result<Profile> {
    let profile = require_profile(state, session).await?;
    if !profile.is_admin {
        return Err(AppError::forbidden("Administrator access is required"));
    }
    Ok(profile)
}

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Json(credentials): Json<Credentials>,
) -> Result<Json<serde_json::Value>> {
    if credentials.username.len() > 48 || credentials.password.len() > 512 {
        return Err(AppError::bad("Invalid credentials"));
    }
    {
        let mut attempts = state.login_attempts.lock().await;
        let instant = Instant::now();
        attempts.retain(|_, entries| {
            entries.retain(|entry| instant.duration_since(*entry) < Duration::from_secs(60));
            !entries.is_empty()
        });
        let global_count: usize = attempts.values().map(Vec::len).sum();
        let entries = attempts
            .entry(credentials.username.to_ascii_lowercase())
            .or_default();
        if entries.len() >= 5 || global_count >= 30 {
            return Err(AppError(
                StatusCode::TOO_MANY_REQUESTS,
                "login_rate_limited",
                "Too many sign-in attempts. Wait a minute and try again.".into(),
            ));
        }
        entries.push(instant);
    }
    let stored: Option<(String, String, i64, bool)> = sqlx::query_as(
        "SELECT id,password_hash,auth_version,enabled FROM profiles WHERE username=?",
    )
    .bind(&credentials.username)
    .fetch_optional(&state.database)
    .await?;
    let password_hash = stored
        .as_ref()
        .map(|row| row.1.clone())
        .unwrap_or_else(|| state.dummy_password_hash.to_string());
    let permit = state
        .password_work
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::bad("Server is stopping"))?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&password_hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(credentials.password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false);
    let Some((profile_id, _, auth_version, enabled)) = stored.filter(|_| valid) else {
        return Err(AppError(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Incorrect username or password".into(),
        ));
    };
    if !enabled {
        return Err(AppError(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Incorrect username or password".into(),
        ));
    }
    session.cycle_id().await?;
    session.insert("profile_id", profile_id).await?;
    session.insert("auth_version", auth_version).await?;
    session.insert("csrf_token", new_id()).await?;
    current(State(state), session).await
}

pub async fn current(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<serde_json::Value>> {
    let profile = require_profile(&state, &session).await?;
    Ok(Json(
        serde_json::json!({"profile":profile, "csrf_token": session.get::<String>("csrf_token").await?}),
    ))
}

pub async fn logout(session: Session) -> Result<StatusCode> {
    session.flush().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Preferences {
    playback_rate: f64,
}
pub async fn preferences(
    State(state): State<AppState>,
    session: Session,
    Json(preferences): Json<Preferences>,
) -> Result<StatusCode> {
    let profile = require_profile(&state, &session).await?;
    if ![0.75, 1.0, 1.25, 1.5, 1.75, 2.0].contains(&preferences.playback_rate) {
        return Err(AppError::bad("Unsupported playback rate"));
    }
    sqlx::query("UPDATE profiles SET playback_rate=? WHERE id=?")
        .bind(preferences.playback_rate)
        .bind(profile.id)
        .execute(&state.database)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn profiles(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<Vec<Profile>>> {
    require_admin(&state, &session).await?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM profiles ORDER BY username")
            .fetch_all(&state.database)
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct NewProfile {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}
pub async fn insert_profile(state: &AppState, input: NewProfile) -> Result<String> {
    validate_credentials(&input.username, &input.password)?;
    let permit = state
        .password_work
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::bad("Server is stopping"))?;
    let password_hash = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_password(&input.password)
    })
    .await
    .map_err(|_| AppError::bad("Password hashing failed"))?
    .map_err(|_| AppError::bad("Password hashing failed"))?;
    let profile_id = new_id();
    let inserted = sqlx::query("INSERT INTO profiles(id,username,password_hash,is_admin) VALUES(?,?,?,?) ON CONFLICT(username) DO NOTHING")
        .bind(&profile_id).bind(input.username).bind(password_hash).bind(input.is_admin).execute(&state.database).await?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::conflict("That username already exists"));
    }
    Ok(profile_id)
}
pub async fn create_profile(
    State(state): State<AppState>,
    session: Session,
    Json(input): Json<NewProfile>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    require_admin(&state, &session).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": insert_profile(&state,input).await?})),
    ))
}

#[derive(Deserialize)]
pub struct ProfileUpdate {
    pub enabled: Option<bool>,
    pub password: Option<String>,
}
pub async fn change_profile(
    state: &AppState,
    profile_id: &str,
    input: ProfileUpdate,
) -> Result<()> {
    let password_hash = if let Some(password) = input.password {
        validate_credentials("valid", &password)?;
        let permit = state
            .password_work
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::bad("Server is stopping"))?;
        Some(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                hash_password(&password)
            })
            .await
            .map_err(|_| AppError::bad("Password hashing failed"))?
            .map_err(|_| AppError::bad("Password hashing failed"))?,
        )
    } else {
        None
    };
    let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
    let profile: Profile = sqlx::query_as("SELECT * FROM profiles WHERE id=?")
        .bind(profile_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(AppError::missing)?;
    if profile.is_admin && input.enabled == Some(false) {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM profiles WHERE is_admin=1 AND enabled=1 AND id<>?",
        )
        .bind(profile_id)
        .fetch_one(&mut *transaction)
        .await?;
        if count == 0 {
            return Err(AppError::bad("Keep at least one enabled administrator"));
        }
    }
    sqlx::query("UPDATE profiles SET enabled=COALESCE(?,enabled),password_hash=COALESCE(?,password_hash),auth_version=auth_version+1 WHERE id=?")
        .bind(input.enabled).bind(password_hash).bind(profile_id).execute(&mut *transaction).await?;
    sqlx::query("DELETE FROM playback_sessions WHERE profile_id=?")
        .bind(profile_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub async fn update_profile(
    State(state): State<AppState>,
    session: Session,
    Path(profile_id): Path<String>,
    Json(input): Json<ProfileUpdate>,
) -> Result<StatusCode> {
    require_admin(&state, &session).await?;
    change_profile(&state, &profile_id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}
