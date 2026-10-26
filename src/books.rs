use crate::{
    AppError, AppState, Result,
    auth::{require_admin, require_profile},
    playback::Position,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tower_sessions::Session;

#[derive(Serialize, FromRow)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub narrator: String,
    pub publication_year: String,
    pub language: String,
    pub description: String,
    pub has_audio: bool,
    pub has_ebook: bool,
    pub available: bool,
    pub has_cover: bool,
    #[serde(skip)]
    pub metadata_overrides: String,
}
impl Book {
    fn apply_overrides(mut self) -> Self {
        if let Ok(overrides) = serde_json::from_str::<serde_json::Value>(&self.metadata_overrides) {
            for (name, target) in [
                ("title", &mut self.title),
                ("author", &mut self.author),
                ("narrator", &mut self.narrator),
                ("publication_year", &mut self.publication_year),
                ("language", &mut self.language),
                ("description", &mut self.description),
            ] {
                if let Some(value) = overrides.get(name).and_then(|value| value.as_str()) {
                    *target = value.to_string();
                }
            }
        }
        self
    }
}
const BOOK_COLUMNS: &str = "b.id,b.title,b.author,b.narrator,b.publication_year,b.language,b.description,b.has_audio,b.has_ebook,b.available,b.cover IS NOT NULL AS has_cover,b.metadata_overrides";

#[derive(Deserialize, Default)]
pub struct LibraryQuery {
    pub search: Option<String>,
    pub kind: Option<String>,
    pub offset: Option<u32>,
    pub limit: Option<u32>,
    pub sort: Option<String>,
}
pub async fn list(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<serde_json::Value>> {
    let profile = require_profile(&state, &session).await?;
    let search = query.search.unwrap_or_default();
    if search.len() > 200 {
        return Err(AppError::bad("Search is too long"));
    }
    let kind = query.kind.unwrap_or_default();
    if !["", "audio", "ebook"].contains(&kind.as_str()) {
        return Err(AppError::bad("Unknown media filter"));
    }
    let recent = query.sort.as_deref() == Some("recent");
    let filter = "WHERE (instr(lower(COALESCE(json_extract(b.metadata_overrides,'$.title'),b.title)),lower(?))>0 OR instr(lower(COALESCE(json_extract(b.metadata_overrides,'$.author'),b.author)),lower(?))>0) AND (?='' OR (?='audio' AND b.has_audio=1) OR (?='ebook' AND b.has_ebook=1)) AND (?=0 OR p.updated_at IS NOT NULL)";
    let order = if recent {
        "p.updated_at DESC,b.id"
    } else {
        "COALESCE(json_extract(b.metadata_overrides,'$.title'),b.title) COLLATE NOCASE,b.id"
    };
    let sql = format!(
        "SELECT {BOOK_COLUMNS} FROM books b LEFT JOIN playback_positions p ON p.book_id=b.id AND p.profile_id=? {filter} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    // Only static column/filter/order fragments are interpolated. Every user
    // value is a bound parameter, including pagination and the profile ID.
    let books: Vec<Book> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(&profile.id)
        .bind(&search)
        .bind(&search)
        .bind(&kind)
        .bind(&kind)
        .bind(&kind)
        .bind(recent)
        .bind(query.limit.unwrap_or(50).clamp(1, 50))
        .bind(query.offset.unwrap_or(0))
        .fetch_all(&state.database)
        .await?;
    let count_sql = format!(
        "SELECT COUNT(*) FROM books b LEFT JOIN playback_positions p ON p.book_id=b.id AND p.profile_id=? {filter}"
    );
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(count_sql))
        .bind(&profile.id)
        .bind(&search)
        .bind(&search)
        .bind(&kind)
        .bind(&kind)
        .bind(&kind)
        .bind(recent)
        .fetch_one(&state.database)
        .await?;
    Ok(Json(
        serde_json::json!({"books":books.into_iter().map(Book::apply_overrides).collect::<Vec<_>>(),"total":total}),
    ))
}

#[derive(Serialize, FromRow)]
pub struct MediaFile {
    pub id: String,
    pub kind: String,
    pub content_type: String,
    pub title: String,
    pub duration_seconds: f64,
    pub available: bool,
    #[serde(skip)]
    pub chapters_json: String,
    #[sqlx(skip)]
    pub chapters: serde_json::Value,
}
pub async fn detail(
    State(state): State<AppState>,
    session: Session,
    Path(book_id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let profile = require_profile(&state, &session).await?;
    let book: Book = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {BOOK_COLUMNS} FROM books b WHERE b.id=?"
    )))
    .bind(&book_id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(AppError::missing)?;
    let mut files: Vec<MediaFile> = sqlx::query_as("SELECT m.*,EXISTS(SELECT 1 FROM file_locations l WHERE l.media_file_id=m.id AND l.available=1) AS available FROM media_files m WHERE book_id=? ORDER BY COALESCE(custom_order,sort_order),id")
        .bind(&book_id).fetch_all(&state.database).await?;
    for file in &mut files {
        file.chapters =
            serde_json::from_str(&file.chapters_json).unwrap_or_else(|_| serde_json::json!([]));
    }
    let position: Option<Position> = sqlx::query_as("SELECT media_file_id,playback_position_seconds,updated_at FROM playback_positions WHERE profile_id=? AND book_id=?")
        .bind(profile.id).bind(&book_id).fetch_optional(&state.database).await?;
    Ok(Json(
        serde_json::json!({"book":book.apply_overrides(),"files":files,"position":position}),
    ))
}
pub async fn cover(
    State(state): State<AppState>,
    session: Session,
    Path(book_id): Path<String>,
) -> Result<Response> {
    require_profile(&state, &session).await?;
    let cover: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT cover FROM books WHERE id=?")
        .bind(book_id)
        .fetch_optional(&state.database)
        .await?;
    Ok((
        [
            ("content-type", "image/jpeg"),
            ("cache-control", "private, max-age=300"),
        ],
        cover.flatten().ok_or_else(AppError::missing)?,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct BookUpdate {
    pub metadata: Option<std::collections::BTreeMap<String, String>>,
    pub audio_file_order: Option<Vec<String>>,
}
pub async fn update(
    State(state): State<AppState>,
    session: Session,
    Path(book_id): Path<String>,
    Json(input): Json<BookUpdate>,
) -> Result<StatusCode> {
    require_admin(&state, &session).await?;
    let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
    let existing: String = sqlx::query_scalar("SELECT metadata_overrides FROM books WHERE id=?")
        .bind(&book_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(AppError::missing)?;
    if let Some(metadata) = input.metadata {
        let mut overrides: serde_json::Value =
            serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({}));
        for (key, value) in metadata {
            if ![
                "title",
                "author",
                "narrator",
                "publication_year",
                "language",
                "description",
            ]
            .contains(&key.as_str())
                || value.len() > if key == "description" { 8000 } else { 500 }
            {
                return Err(AppError::bad("Unknown metadata field or value too long"));
            }
            if key == "title" && value.trim().is_empty() {
                return Err(AppError::bad("A title is required"));
            }
            overrides[&key] = value.trim().into();
        }
        sqlx::query("UPDATE books SET metadata_overrides=? WHERE id=?")
            .bind(overrides.to_string())
            .bind(&book_id)
            .execute(&mut *transaction)
            .await?;
    }
    if let Some(order) = input.audio_file_order {
        let files: Vec<String> =
            sqlx::query_scalar("SELECT id FROM media_files WHERE book_id=? AND kind='audio'")
                .bind(&book_id)
                .fetch_all(&mut *transaction)
                .await?;
        let unique: std::collections::HashSet<_> = order.iter().collect();
        if unique.len() != order.len()
            || files.len() != order.len()
            || files.iter().any(|file| !unique.contains(file))
        {
            return Err(AppError::bad(
                "Include each of this book's audio files exactly once",
            ));
        }
        for (index, file_id) in order.iter().enumerate() {
            sqlx::query("UPDATE media_files SET custom_order=? WHERE id=?")
                .bind(index as i64)
                .bind(file_id)
                .execute(&mut *transaction)
                .await?;
        }
    }
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
