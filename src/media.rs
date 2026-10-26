use crate::{AppError, AppState, Result, auth::require_profile};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{fs::File, path::Path as FilePath};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tower_sessions::Session;

pub fn open_library_file(library: &FilePath, relative_path: &str) -> std::io::Result<File> {
    let root = File::open(library)?;
    let descriptor = rustix::fs::openat2(
        &root,
        relative_path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
    )?;
    let file = File::from(descriptor);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("Not a regular file"));
    }
    Ok(file)
}

pub fn byte_range(value: &str, length: u64) -> Option<(u64, u64)> {
    let range = value.strip_prefix("bytes=")?;
    if range.contains(',') || length == 0 {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    if start.is_empty() {
        let count: u64 = end.parse().ok()?;
        return (count > 0).then(|| (length.saturating_sub(count), length - 1));
    }
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() {
        length - 1
    } else {
        end.parse::<u64>().ok()?.min(length - 1)
    };
    (start < length && start <= end).then_some((start, end))
}

pub async fn serve(
    State(state): State<AppState>,
    session: Session,
    Path(media_file_id): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response> {
    require_profile(&state, &session).await?;
    let locations: Vec<(String,i64,String,String,String)> = sqlx::query_as("SELECT l.relative_path,l.byte_length,l.modified_ns,m.content_type,m.kind FROM file_locations l JOIN media_files m ON m.id=l.media_file_id WHERE m.id=? AND l.available=1 ORDER BY l.relative_path")
        .bind(media_file_id).fetch_all(&state.database).await?;
    let mut selected = None;
    for (relative_path, expected_length, expected_modified, content_type, kind) in locations {
        if let Ok(file) = open_library_file(&state.config.library_directory, &relative_path)
            && let Ok(metadata) = file.metadata()
        {
            let modified = crate::index::modified_ns(&metadata);
            if metadata.len() == expected_length as u64 && modified == expected_modified {
                selected = Some((file, content_type, kind));
                break;
            }
        }
    }
    let (file, content_type, kind) = selected.ok_or_else(AppError::missing)?;
    let length = file.metadata().map_err(|_| AppError::missing())?.len();
    let range = if let Some(value) = headers.get(header::RANGE) {
        match value
            .to_str()
            .ok()
            .and_then(|value| byte_range(value, length))
        {
            Some(range) => Some(range),
            None => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [("content-range", format!("bytes */{length}"))],
                )
                    .into_response());
            }
        }
    } else {
        None
    };
    let (start, end) = range.unwrap_or((0, length.saturating_sub(1)));
    let response_length = if length == 0 { 0 } else { end - start + 1 };
    let mut file = tokio::fs::File::from_std(file);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|_| AppError::missing())?;
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::with_capacity(
            file.take(response_length),
            64 * 1024,
        ))
    };
    let mut response = Response::new(body);
    *response.status_mut() = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        content_type.parse().map_err(|_| AppError::missing())?,
    );
    response_headers.insert(header::CONTENT_LENGTH, response_length.into());
    response_headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store, no-transform"),
    );
    if range.is_some() {
        response_headers.insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}")
                .parse()
                .map_err(|_| AppError::missing())?,
        );
    }
    if kind == "ebook" {
        response_headers.insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static("attachment"),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_contract() {
        assert_eq!(byte_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(byte_range("bytes=5-", 10), Some((5, 9)));
        assert_eq!(byte_range("bytes=-20", 10), Some((0, 9)));
        assert_eq!(byte_range("bytes=2-100", 10), Some((2, 9)));
        for value in [
            "bytes=10-",
            "bytes=3-2",
            "bytes=0-1,3-4",
            "bytes=-0",
            "items=0-1",
            "bytes=18446744073709551616-",
        ] {
            assert_eq!(byte_range(value, 10), None);
        }
    }
    #[test]
    fn files_cannot_escape_library() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("book.mp3"), b"audio").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", directory.path().join("escape")).unwrap();
        assert!(open_library_file(directory.path(), "book.mp3").is_ok());
        for path in ["escape", "../etc/passwd", "/etc/passwd", "."] {
            assert!(open_library_file(directory.path(), path).is_err());
        }
    }
}
