use crate::{
    AppState, Result, ScanStatus, auth::require_admin, media::open_library_file,
    metadata::Metadata, new_id, now,
};
use anyhow::{Context, ensure};
use axum::{Json, extract::State, http::StatusCode};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
    process::Stdio,
    time::{Duration, UNIX_EPOCH},
};
use tower_sessions::Session;

#[derive(Clone)]
struct FileObservation {
    relative_path: String,
    byte_length: u64,
    modified_ns: String,
    extension: String,
}
#[derive(Clone)]
struct CachedFile {
    observation: FileObservation,
    fingerprint: String,
    metadata: Metadata,
}
#[derive(Default)]
pub struct Scanner {
    observed: HashMap<String, (u64, String)>,
    published_groups: HashMap<String, String>,
}

pub fn modified_ns(metadata: &fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string())
        .unwrap_or_default()
}
fn is_audio(extension: &str) -> bool {
    matches!(extension, "mp3" | "m4a" | "m4b")
}
fn is_ebook(extension: &str) -> bool {
    matches!(extension, "epub" | "pdf" | "azw3")
}
fn supported(extension: &str) -> bool {
    is_audio(extension)
        || is_ebook(extension)
        || matches!(extension, "opf" | "jpg" | "jpeg" | "png" | "webp")
}

fn collect_files(root: &Path) -> anyhow::Result<Vec<FileObservation>> {
    ensure!(
        root.is_dir(),
        "Library directory is unavailable; previous index preserved"
    );
    let mut directories = vec![(root.to_path_buf(), 0)];
    let mut files = Vec::new();
    while let Some((directory, depth)) = directories.pop() {
        ensure!(depth < 64, "Directory nesting exceeds 64 levels");
        for entry in fs::read_dir(&directory)
            .context("Cannot read a library directory; previous index preserved")?
        {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !supported(&extension)
                && !matches!(
                    extension.as_str(),
                    "flac" | "ogg" | "opus" | "wav" | "aac" | "mobi" | "m4p"
                )
            {
                continue;
            }
            let metadata = entry.metadata()?;
            let relative_path = path
                .strip_prefix(root)?
                .to_str()
                .context("A library filename is not valid UTF-8")?
                .to_string();
            files.push(FileObservation {
                relative_path,
                byte_length: metadata.len(),
                modified_ns: modified_ns(&metadata),
                extension,
            });
            ensure!(
                files.len() <= 100_000,
                "Library exceeds the 100,000 file limit"
            );
        }
    }
    files.sort_by(|left, right| natural_cmp(&left.relative_path, &right.relative_path));
    Ok(files)
}

pub fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let mut a = String::new();
                let mut b = String::new();
                while left.peek().is_some_and(|c| c.is_ascii_digit()) {
                    a.push(left.next().unwrap());
                }
                while right.peek().is_some_and(|c| c.is_ascii_digit()) {
                    b.push(right.next().unwrap());
                }
                let a = a.trim_start_matches('0');
                let b = b.trim_start_matches('0');
                let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            (Some(_), Some(_)) => {
                let order = left.next().cmp(&right.next());
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
        }
    }
}

async fn inspect_file(state: &AppState, observation: &FileObservation) -> anyhow::Result<Metadata> {
    let file = open_library_file(&state.config.library_directory, &observation.relative_path)?;
    let mut child = tokio::process::Command::new(&state.config.inspector_executable)
        .arg("inspect")
        .arg(&observation.extension)
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("Cannot capture metadata")?;
    let operation = async {
        use tokio::io::AsyncReadExt;
        let mut output = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut output)
            .await?;
        ensure!(
            output.len() <= 4 * 1024 * 1024,
            "Metadata output exceeds its limit"
        );
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "Cannot read this file: invalid, unsupported, or exceeds metadata resource limits"
        );
        Ok::<Metadata, anyhow::Error>(serde_json::from_slice(&output)?)
    };
    tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .context("Metadata inspection timed out")?
}

impl Scanner {
    pub async fn scan(&mut self, state: &AppState) -> anyhow::Result<()> {
        let library = state.config.library_directory.clone();
        let files = tokio::task::spawn_blocking(move || collect_files(&library)).await??;
        let all_paths: HashSet<String> = files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect();
        let mut groups: BTreeMap<String, Vec<FileObservation>> = BTreeMap::new();
        let mut issues = BTreeMap::new();
        for file in &files {
            if !supported(&file.extension) {
                issues.insert(
                    file.relative_path.clone(),
                    "Unsupported format in v0.1".to_string(),
                );
                continue;
            }
            let path = Path::new(&file.relative_path);
            let parent = path.parent().unwrap_or(Path::new(""));
            let group = if parent.as_os_str().is_empty()
                && (is_audio(&file.extension) || is_ebook(&file.extension))
            {
                file.relative_path.clone()
            } else {
                parent.to_string_lossy().to_string()
            };
            groups.entry(group).or_default().push(file.clone());
        }
        let scan_id = new_id();
        self.published_groups
            .retain(|group, _| groups.contains_key(group));
        for (group, mut members) in groups {
            if !members
                .iter()
                .any(|file| is_audio(&file.extension) || is_ebook(&file.extension))
            {
                continue;
            }
            let ready = members.iter().all(|file| {
                self.observed.get(&file.relative_path)
                    == Some(&(file.byte_length, file.modified_ns.clone()))
            });
            if !ready {
                self.published_groups.remove(&group);
                issues.insert(group.clone(), "Waiting for files to finish copying".into());
                continue;
            }
            let observation_key = members
                .iter()
                .map(|file| {
                    format!(
                        "{}:{}:{}",
                        file.relative_path, file.byte_length, file.modified_ns
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if self.published_groups.get(&group) == Some(&observation_key) {
                continue;
            }
            self.published_groups.remove(&group);
            members.sort_by(|left, right| natural_cmp(&left.relative_path, &right.relative_path));
            let mut cached_members = Vec::new();
            let mut failed = false;
            for observation in &members {
                let result = self.read_file(state, observation.clone()).await;
                match result {
                    Ok(cached) => {
                        cached_members.push(cached);
                    }
                    Err(error) => {
                        issues.insert(observation.relative_path.clone(), error.to_string());
                        failed = true;
                    }
                }
            }
            if failed {
                continue;
            }
            match publish_group(state, &group, &cached_members, &scan_id).await {
                Ok(()) => {
                    self.published_groups.insert(group, observation_key);
                }
                Err(error) => {
                    issues.insert(group, error.to_string());
                }
            }
        }
        // Only complete directory enumeration permits missing-file reconciliation.
        let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
        let locations: Vec<String> =
            sqlx::query_scalar("SELECT relative_path FROM file_locations WHERE available=1")
                .fetch_all(&mut *transaction)
                .await?;
        for path in locations {
            if !all_paths.contains(&path) {
                sqlx::query("UPDATE file_locations SET available=0 WHERE relative_path=?")
                    .bind(path)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        sqlx::query("UPDATE books SET available=EXISTS(SELECT 1 FROM media_files m JOIN file_locations l ON l.media_file_id=m.id WHERE m.book_id=books.id AND l.available=1)").execute(&mut *transaction).await?;
        sqlx::query("DELETE FROM index_issues")
            .execute(&mut *transaction)
            .await?;
        for (path, message) in issues {
            sqlx::query("INSERT INTO index_issues VALUES(?,?,?)")
                .bind(path)
                .bind(message)
                .bind(now())
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query("DELETE FROM login_sessions WHERE expires_at <= ?")
            .bind(now())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.observed = files
            .into_iter()
            .map(|file| (file.relative_path, (file.byte_length, file.modified_ns)))
            .collect();
        Ok(())
    }

    async fn read_file(
        &self,
        state: &AppState,
        observation: FileObservation,
    ) -> anyhow::Result<CachedFile> {
        let known:Option<String>=sqlx::query_scalar("SELECT m.fingerprint FROM file_locations l JOIN media_files m ON m.id=l.media_file_id WHERE l.relative_path=? AND l.byte_length=? AND l.modified_ns=?")
            .bind(&observation.relative_path).bind(observation.byte_length as i64).bind(&observation.modified_ns).fetch_optional(&state.database).await?;
        let fingerprint = if let Some(fingerprint) = known {
            fingerprint
        } else {
            let mut file =
                open_library_file(&state.config.library_directory, &observation.relative_path)?;
            tokio::task::spawn_blocking(move || {
                let mut digest = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    digest.update(&buffer[..count]);
                }
                Ok::<String, std::io::Error>(format!("{:x}", digest.finalize()))
            })
            .await??
        };
        let metadata = inspect_file(state, &observation).await?;
        let current =
            open_library_file(&state.config.library_directory, &observation.relative_path)?
                .metadata()?;
        ensure!(
            current.len() == observation.byte_length
                && modified_ns(&current) == observation.modified_ns,
            "File changed during indexing; waiting for a stable copy"
        );
        Ok(CachedFile {
            observation,
            fingerprint,
            metadata,
        })
    }
}

async fn publish_group(
    state: &AppState,
    group: &str,
    files: &[CachedFile],
    scan_id: &str,
) -> anyhow::Result<()> {
    let mut media: Vec<_> = files
        .iter()
        .filter(|file| {
            is_audio(&file.observation.extension) || is_ebook(&file.observation.extension)
        })
        .collect();
    let mut fingerprints: Vec<_> = media.iter().map(|file| file.fingerprint.clone()).collect();
    fingerprints.sort();
    fingerprints.dedup();
    let signature = format!("{:x}", Sha256::digest(fingerprints.join("\n").as_bytes()));
    let has_audio = media
        .iter()
        .any(|file| is_audio(&file.observation.extension));
    let has_ebook = media
        .iter()
        .any(|file| is_ebook(&file.observation.extension));
    let mut metadata = media
        .first()
        .map(|file| file.metadata.clone())
        .unwrap_or_default();
    if let Some(audio) = media
        .iter()
        .find(|file| is_audio(&file.observation.extension))
    {
        metadata = audio.metadata.clone();
    }
    for file in files
        .iter()
        .filter(|file| file.observation.extension == "opf")
    {
        metadata.overlay(&file.metadata);
    }
    if let Some(cover) = files
        .iter()
        .filter(|file| {
            matches!(
                file.observation.extension.as_str(),
                "jpg" | "jpeg" | "png" | "webp"
            )
        })
        .find_map(|file| file.metadata.cover.clone())
    {
        metadata.cover = Some(cover);
    }
    if metadata.title.is_empty() {
        metadata.title = Path::new(group)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into();
    }
    media.sort_by(|left, right| {
        (left.metadata.disc_number, left.metadata.track_number)
            .cmp(&(right.metadata.disc_number, right.metadata.track_number))
            .then_with(|| {
                natural_cmp(
                    &left.observation.relative_path,
                    &right.observation.relative_path,
                )
            })
    });
    let mut transaction = state.database.begin_with("BEGIN IMMEDIATE").await?;
    let exact: Vec<String> = sqlx::query_scalar("SELECT id FROM books WHERE signature=?")
        .bind(&signature)
        .fetch_all(&mut *transaction)
        .await?;
    ensure!(
        exact.len() <= 1,
        "Needs review: multiple books match these files"
    );
    let book_id = if let Some(book_id) = exact.first() {
        book_id.clone()
    } else {
        // Adding another format to an existing book is safe only when the entire old set survives.
        let mut candidates = HashSet::new();
        for fingerprint in &fingerprints {
            let matches: Vec<String> =
                sqlx::query_scalar("SELECT DISTINCT book_id FROM media_files WHERE fingerprint=?")
                    .bind(fingerprint)
                    .fetch_all(&mut *transaction)
                    .await?;
            candidates.extend(matches);
        }
        if candidates.len() == 1 {
            let candidate = candidates.into_iter().next().unwrap();
            let old: Vec<String> =
                sqlx::query_scalar("SELECT fingerprint FROM media_files WHERE book_id=?")
                    .bind(&candidate)
                    .fetch_all(&mut *transaction)
                    .await?;
            ensure!(
                old.iter()
                    .all(|fingerprint| fingerprints.contains(fingerprint)),
                "Needs review: a partial match cannot safely replace an existing book"
            );
            candidate
        } else {
            ensure!(
                candidates.is_empty(),
                "Needs review: these files belong to multiple books"
            );
            new_id()
        }
    };
    sqlx::query("INSERT INTO books(id,signature,title,author,narrator,publication_year,language,description,has_audio,has_ebook,cover,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET signature=excluded.signature,title=excluded.title,author=excluded.author,narrator=excluded.narrator,publication_year=excluded.publication_year,language=excluded.language,description=excluded.description,has_audio=excluded.has_audio,has_ebook=excluded.has_ebook,cover=excluded.cover,updated_at=excluded.updated_at")
        .bind(&book_id).bind(signature).bind(metadata.title).bind(metadata.author).bind(metadata.narrator).bind(metadata.publication_year).bind(metadata.language).bind(metadata.description).bind(has_audio).bind(has_ebook).bind(metadata.cover).bind(now()).execute(&mut *transaction).await?;
    for (index, file) in media.iter().enumerate() {
        let content_type = match file.observation.extension.as_str() {
            "mp3" => "audio/mpeg",
            "m4a" | "m4b" => "audio/mp4",
            "epub" => "application/epub+zip",
            "pdf" => "application/pdf",
            _ => "application/vnd.amazon.ebook",
        };
        let media_file_id:String=sqlx::query_scalar("INSERT INTO media_files(id,book_id,fingerprint,kind,content_type,duration_seconds,track_number,disc_number,sort_order,title,chapters_json) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(book_id,fingerprint) DO UPDATE SET duration_seconds=excluded.duration_seconds,chapters_json=excluded.chapters_json RETURNING id")
            .bind(new_id()).bind(&book_id).bind(&file.fingerprint).bind(if is_audio(&file.observation.extension){"audio"}else{"ebook"}).bind(content_type).bind(file.metadata.duration_seconds).bind(file.metadata.track_number).bind(file.metadata.disc_number).bind(index as i64)
            .bind(Path::new(&file.observation.relative_path).file_stem().unwrap_or_default().to_string_lossy().to_string()).bind(serde_json::to_string(&file.metadata.chapters)?).fetch_one(&mut *transaction).await?;
        sqlx::query("INSERT INTO file_locations(relative_path,media_file_id,byte_length,modified_ns,available,scan_id) VALUES(?,?,?,?,1,?) ON CONFLICT(relative_path) DO UPDATE SET media_file_id=excluded.media_file_id,byte_length=excluded.byte_length,modified_ns=excluded.modified_ns,available=1,scan_id=excluded.scan_id")
            .bind(&file.observation.relative_path).bind(media_file_id).bind(file.observation.byte_length as i64).bind(&file.observation.modified_ns).bind(scan_id).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn run(state: AppState) {
    let mut scanner = Scanner::default();
    loop {
        state.scan_status.lock().await.running = true;
        match scanner.scan(&state).await {
            Ok(()) => {
                *state.scan_status.lock().await = ScanStatus {
                    running: false,
                    last_completed_at: Some(now()),
                    message: "Library scan completed".into(),
                };
            }
            Err(error) => {
                tracing::warn!(error=%error,"library scan failed");
                let mut status = state.scan_status.lock().await;
                status.running = false;
                status.message = error.to_string();
            }
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(state.config.scan_interval_seconds))=>{},_=state.scan_requested.notified()=>{}}
    }
}
pub async fn status(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<serde_json::Value>> {
    require_admin(&state, &session).await?;
    let issues: Vec<(String, String)> = sqlx::query_as(
        "SELECT relative_path,message FROM index_issues ORDER BY relative_path LIMIT 200",
    )
    .fetch_all(&state.database)
    .await?;
    let issue_count: i64 = sqlx::query_scalar("SELECT count(*) FROM index_issues")
        .fetch_one(&state.database)
        .await?;
    Ok(Json(
        serde_json::json!({"scan":state.scan_status.lock().await.clone(),"issues":issues.into_iter().map(|(path,message)|serde_json::json!({"path":path,"message":message})).collect::<Vec<_>>(),"issue_count":issue_count}),
    ))
}
pub async fn request_scan(State(state): State<AppState>, session: Session) -> Result<StatusCode> {
    require_admin(&state, &session).await?;
    state.scan_requested.notify_one();
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_order_is_numeric() {
        let mut names = vec!["Part 10", "Part 2", "Part 01"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["Part 01", "Part 2", "Part 10"]);
    }
}
