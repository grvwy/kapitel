use sqlx::SqlitePool;
use tower_sessions::{
    SessionStore,
    session::{Id, Record},
    session_store::{Error, Result},
};

// Session IDs, cookies and lifecycle remain owned by tower-sessions. This small
// adapter keeps the database on the supported SQLx version without another ORM.
#[derive(Clone, Debug)]
pub struct SqliteSessionStore(pub SqlitePool);

fn database_error(error: sqlx::Error) -> Error {
    Error::Backend(error.to_string())
}

#[async_trait::async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create(&self, record: &mut Record) -> Result<()> {
        let encoded =
            serde_json::to_string(record).map_err(|error| Error::Encode(error.to_string()))?;
        // A collision fails closed; it must never overwrite another login.
        sqlx::query("INSERT INTO login_sessions(session_id,record_json,expires_at) VALUES(?,?,?)")
            .bind(record.id.to_string())
            .bind(encoded)
            .bind(record.expiry_date.unix_timestamp())
            .execute(&self.0)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    async fn save(&self, record: &Record) -> Result<()> {
        let encoded =
            serde_json::to_string(record).map_err(|error| Error::Encode(error.to_string()))?;
        // Updating, rather than upserting, cannot resurrect a logged-out session.
        sqlx::query("UPDATE login_sessions SET record_json=?,expires_at=? WHERE session_id=?")
            .bind(encoded)
            .bind(record.expiry_date.unix_timestamp())
            .bind(record.id.to_string())
            .execute(&self.0)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> Result<Option<Record>> {
        let encoded: Option<String> = sqlx::query_scalar(
            "SELECT record_json FROM login_sessions WHERE session_id=? AND expires_at>?",
        )
        .bind(session_id.to_string())
        .bind(crate::now())
        .fetch_optional(&self.0)
        .await
        .map_err(database_error)?;
        encoded
            .map(|encoded| {
                serde_json::from_str(&encoded).map_err(|error| Error::Decode(error.to_string()))
            })
            .transpose()
    }

    async fn delete(&self, session_id: &Id) -> Result<()> {
        sqlx::query("DELETE FROM login_sessions WHERE session_id=?")
            .bind(session_id.to_string())
            .execute(&self.0)
            .await
            .map_err(database_error)?;
        Ok(())
    }
}
