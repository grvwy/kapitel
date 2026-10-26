use crate::{AppError, AppState, Result, auth::require_profile, new_id, now};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tower_sessions::Session;

#[derive(Serialize, FromRow)]
pub struct Position {
    pub media_file_id: String,
    pub playback_position_seconds: f64,
    pub updated_at: i64,
}
#[derive(Deserialize)]
pub struct StartPlayback {
    pub book_id: String,
    #[serde(default)]
    pub restart: bool,
}
pub async fn start(
    State(state): State<AppState>,
    session: Session,
    Json(input): Json<StartPlayback>,
) -> Result<Json<serde_json::Value>> {
    let profile = require_profile(&state, &session).await?;
    let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
    let first_file: Option<String> = sqlx::query_scalar("SELECT m.id FROM media_files m WHERE m.book_id=? AND m.kind='audio' AND EXISTS(SELECT 1 FROM file_locations l WHERE l.media_file_id=m.id AND l.available=1) ORDER BY COALESCE(m.custom_order,m.sort_order),m.id LIMIT 1")
        .bind(&input.book_id).fetch_optional(&mut *transaction).await?;
    let first_file = first_file.ok_or_else(|| AppError::bad("This book has no available audio"))?;
    let position: Option<Position> = sqlx::query_as("SELECT media_file_id,playback_position_seconds,updated_at FROM playback_positions WHERE profile_id=? AND book_id=?")
        .bind(&profile.id).bind(&input.book_id).fetch_optional(&mut *transaction).await?;
    let position = if input.restart { None } else { position }.unwrap_or(Position {
        media_file_id: first_file,
        playback_position_seconds: 0.0,
        updated_at: now(),
    });
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM file_locations WHERE media_file_id=? AND available=1)",
    )
    .bind(&position.media_file_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !available {
        return Err(AppError::conflict(
            "The file at your saved position is missing. Restore it or choose Start over.",
        ));
    }
    let playback_session_id = new_id();
    sqlx::query("INSERT INTO playback_sessions(profile_id,session_id,book_id,last_sequence_number) VALUES(?,?,?,0) ON CONFLICT(profile_id) DO UPDATE SET session_id=excluded.session_id,book_id=excluded.book_id,last_sequence_number=0")
        .bind(&profile.id).bind(&playback_session_id).bind(&input.book_id).execute(&mut *transaction).await?;
    if input.restart {
        sqlx::query("INSERT INTO playback_positions(profile_id,book_id,media_file_id,playback_position_seconds,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(profile_id,book_id) DO UPDATE SET media_file_id=excluded.media_file_id,playback_position_seconds=excluded.playback_position_seconds,updated_at=excluded.updated_at")
            .bind(&profile.id).bind(&input.book_id).bind(&position.media_file_id).bind(0.0).bind(now()).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"playback_session_id":playback_session_id,"position":position}),
    ))
}

#[derive(Deserialize, Clone, Serialize)]
pub struct PositionUpdate {
    pub media_file_id: String,
    pub playback_position_seconds: f64,
    pub sequence_number: i64,
}
pub async fn save(
    State(state): State<AppState>,
    session: Session,
    Path(playback_session_id): Path<String>,
    Json(input): Json<PositionUpdate>,
) -> Result<Json<serde_json::Value>> {
    let profile = require_profile(&state, &session).await?;
    if !input.playback_position_seconds.is_finite()
        || input.playback_position_seconds < 0.0
        || input.sequence_number < 1
    {
        return Err(AppError::bad(
            "Invalid playback position or sequence number",
        ));
    }
    let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
    let active: Option<(String, i64)> = sqlx::query_as("SELECT book_id,last_sequence_number FROM playback_sessions WHERE profile_id=? AND session_id=?")
        .bind(&profile.id).bind(&playback_session_id).fetch_optional(&mut *transaction).await?;
    let (book_id, last_sequence_number) = active.ok_or_else(|| {
        AppError(
            StatusCode::CONFLICT,
            "session_replaced",
            "Playback has moved to another session. Press Resume to take over.".into(),
        )
    })?;
    if input.sequence_number < last_sequence_number {
        return Err(AppError(
            StatusCode::CONFLICT,
            "stale_update",
            "A newer playback position has already been saved".into(),
        ));
    }
    if input.sequence_number == last_sequence_number {
        let previous: Position = sqlx::query_as("SELECT media_file_id,playback_position_seconds,updated_at FROM playback_positions WHERE profile_id=? AND book_id=?")
            .bind(&profile.id).bind(&book_id).fetch_one(&mut *transaction).await?;
        if previous.media_file_id != input.media_file_id
            || previous.playback_position_seconds != input.playback_position_seconds
        {
            return Err(AppError::conflict(
                "A sequence number cannot be reused for a different position",
            ));
        }
        return Ok(Json(
            serde_json::json!({"saved":true,"sequence_number":input.sequence_number}),
        ));
    }
    let duration: Option<f64> = sqlx::query_scalar(
        "SELECT duration_seconds FROM media_files WHERE id=? AND book_id=? AND kind='audio'",
    )
    .bind(&input.media_file_id)
    .bind(&book_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let duration =
        duration.ok_or_else(|| AppError::bad("The audio file does not belong to this book"))?;
    if input.playback_position_seconds > duration + 5.0 {
        return Err(AppError::bad("Playback position exceeds the file duration"));
    }
    sqlx::query("INSERT INTO playback_positions(profile_id,book_id,media_file_id,playback_position_seconds,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(profile_id,book_id) DO UPDATE SET media_file_id=excluded.media_file_id,playback_position_seconds=excluded.playback_position_seconds,updated_at=excluded.updated_at")
        .bind(&profile.id).bind(&book_id).bind(&input.media_file_id).bind(input.playback_position_seconds).bind(now()).execute(&mut *transaction).await?;
    sqlx::query("UPDATE playback_sessions SET last_sequence_number=? WHERE profile_id=?")
        .bind(input.sequence_number)
        .bind(&profile.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Json(
        serde_json::json!({"saved":true,"sequence_number":input.sequence_number}),
    ))
}
