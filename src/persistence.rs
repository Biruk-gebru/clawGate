use sqlx::SqlitePool;
use crate::dashboard::BackendInfo;

#[derive(sqlx::FromRow)]
pub struct PersistedBackend {
    pub url: String,
    pub request_count: i64,
    pub error_count: i64,
    pub failed_count: i64,
    pub circuit_state: String,
}

pub async fn init_db(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS backend_state (
            url TEXT PRIMARY KEY,
            request_count INTEGER NOT NULL DEFAULT 0,
            error_count   INTEGER NOT NULL DEFAULT 0,
            failed_count  INTEGER NOT NULL DEFAULT 0,
            circuit_state TEXT NOT NULL DEFAULT 'closed'
        )"
    )
    .execute(pool)
    .await
    .expect("Failed to create backend_state table");
}

pub async fn load_state(pool: &SqlitePool) -> Vec<PersistedBackend> {
    sqlx::query_as::<_, PersistedBackend>("SELECT * FROM backend_state")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

pub async fn save_state(pool: &SqlitePool, backends: &[BackendInfo]) {
    sqlx::query("DELETE FROM backend_state")
        .execute(pool)
        .await
        .expect("Failed to clear backend_state");

    for b in backends {
        sqlx::query(
            "INSERT INTO backend_state (url, request_count, error_count, failed_count, circuit_state)
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&b.url)
        .bind(b.request_count as i64)
        .bind(b.error_count as i64)
        .bind(b.failed_count as i64)
        .bind(b.circuit_state.to_db_string())
        .execute(pool)
        .await
        .expect("Failed to insert backend state");
    }
}
