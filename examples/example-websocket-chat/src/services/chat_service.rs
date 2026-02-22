use r2e::prelude::*;
use sqlx::SqlitePool;

use crate::models::StoredMessage;

#[derive(Clone)]
pub struct ChatService {
    pool: SqlitePool,
}

#[bean]
impl ChatService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn save_message(
        &self,
        room: &str,
        username: &str,
        text: &str,
    ) -> Result<(), HttpError> {
        sqlx::query("INSERT INTO messages (room, username, text) VALUES (?, ?, ?)")
            .bind(room)
            .bind(username)
            .bind(text)
            .execute(&self.pool)
            .await
            .map_err(|e| HttpError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn get_history(
        &self,
        room: &str,
        limit: i64,
    ) -> Result<Vec<StoredMessage>, HttpError> {
        let messages = sqlx::query_as::<_, StoredMessage>(
            "SELECT id, room, username, text FROM messages WHERE room = ? \
             ORDER BY id DESC LIMIT ?",
        )
        .bind(room)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| HttpError::Internal(e.to_string()))?;

        Ok(messages.into_iter().rev().collect())
    }

    pub async fn list_rooms(&self) -> Result<Vec<String>, HttpError> {
        let rooms: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT room FROM messages ORDER BY room")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| HttpError::Internal(e.to_string()))?;

        Ok(rooms.into_iter().map(|(r,)| r).collect())
    }
}
