CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM schema_version);
CREATE TABLE IF NOT EXISTS profiles (
 id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE COLLATE NOCASE,
 password_hash TEXT NOT NULL, is_admin INTEGER NOT NULL DEFAULT 0,
 enabled INTEGER NOT NULL DEFAULT 1, auth_version INTEGER NOT NULL DEFAULT 1,
 playback_rate REAL NOT NULL DEFAULT 1.0
);
CREATE TABLE IF NOT EXISTS books (
 id TEXT PRIMARY KEY, signature TEXT NOT NULL, title TEXT NOT NULL,
 author TEXT NOT NULL DEFAULT '', narrator TEXT NOT NULL DEFAULT '',
 publication_year TEXT NOT NULL DEFAULT '', language TEXT NOT NULL DEFAULT '',
 description TEXT NOT NULL DEFAULT '', metadata_overrides TEXT NOT NULL DEFAULT '{}',
 has_audio INTEGER NOT NULL DEFAULT 0, has_ebook INTEGER NOT NULL DEFAULT 0,
 available INTEGER NOT NULL DEFAULT 1, cover BLOB, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS books_signature ON books(signature);
CREATE TABLE IF NOT EXISTS media_files (
 id TEXT PRIMARY KEY, book_id TEXT NOT NULL REFERENCES books(id), fingerprint TEXT NOT NULL,
 kind TEXT NOT NULL, content_type TEXT NOT NULL, duration_seconds REAL NOT NULL DEFAULT 0,
 track_number INTEGER NOT NULL DEFAULT 0, disc_number INTEGER NOT NULL DEFAULT 0,
 sort_order INTEGER NOT NULL, custom_order INTEGER,
 title TEXT NOT NULL DEFAULT '', chapters_json TEXT NOT NULL DEFAULT '[]',
 UNIQUE(book_id, fingerprint)
);
CREATE TABLE IF NOT EXISTS file_locations (
 relative_path TEXT PRIMARY KEY, media_file_id TEXT NOT NULL REFERENCES media_files(id),
 byte_length INTEGER NOT NULL, modified_ns TEXT NOT NULL, available INTEGER NOT NULL DEFAULT 1,
 scan_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS location_media ON file_locations(media_file_id, available);
CREATE TABLE IF NOT EXISTS playback_positions (
 profile_id TEXT NOT NULL REFERENCES profiles(id), book_id TEXT NOT NULL REFERENCES books(id),
 media_file_id TEXT NOT NULL REFERENCES media_files(id), playback_position_seconds REAL NOT NULL,
 updated_at INTEGER NOT NULL, PRIMARY KEY(profile_id, book_id)
);
CREATE TABLE IF NOT EXISTS playback_sessions (
 profile_id TEXT PRIMARY KEY REFERENCES profiles(id), session_id TEXT NOT NULL UNIQUE,
 book_id TEXT NOT NULL REFERENCES books(id), last_sequence_number INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS index_issues (
 relative_path TEXT PRIMARY KEY, message TEXT NOT NULL, observed_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS login_sessions (
 session_id TEXT PRIMARY KEY, record_json TEXT NOT NULL, expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS login_session_expiry ON login_sessions(expires_at);
