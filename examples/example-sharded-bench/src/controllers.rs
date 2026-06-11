use r2e::prelude::*;
use serde::Serialize;

use crate::state::Services;

/// Small JSON payload returned by `/json`. A few fields, representative of a
/// typical small API response where serialization — not IO — dominates.
#[derive(Serialize)]
struct Message {
    id: u32,
    message: &'static str,
    ok: bool,
}

/// A single row read from sqlite by `/db`.
#[derive(Serialize)]
struct Row {
    id: i64,
    name: String,
    value: i64,
}

#[derive(Controller)]
#[controller(path = "/", state = Services)]
pub struct BenchController {
    #[inject]
    pool: sqlx::SqlitePool,
}

#[routes]
impl BenchController {
    /// Plaintext endpoint — pure HTTP serving, no serialization.
    #[get("/plain")]
    async fn plain(&self) -> &'static str {
        "Hello, World!"
    }

    /// Small serialized JSON object.
    #[get("/json")]
    async fn json(&self) -> Json<Message> {
        Json(Message {
            id: 1,
            message: "Hello, World!",
            ok: true,
        })
    }

    /// One sqlite SELECT by id, returning a small JSON row.
    ///
    /// The id is derived from the current time so the query is not a single
    /// degenerate constant lookup. (All 100 tiny rows fit in one sqlite page,
    /// so this does not change cache behavior — it only varies the bound value.)
    #[get("/db")]
    async fn db(&self) -> Json<Row> {
        let id = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            % 100
            + 1) as i64;

        let (id, name, value): (i64, String, i64) =
            sqlx::query_as("SELECT id, name, value FROM items WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .expect("seeded row must exist");

        Json(Row { id, name, value })
    }
}
