use anyhow::{Context, Result};
use sqlx::SqliteConnection;
use uuid::Uuid;

/// Called inside the enqueue transaction, after its first write has acquired the lock.
pub(super) async fn resolve(
    connection: &mut SqliteConnection,
    source_key: &str,
    created_at: i64,
) -> Result<Uuid> {
    crate::ingest::validate_source_key(source_key)?;
    let filename = source_key.rsplit('/').next().context("source_key vacía")?;
    sqlx::query(
        "INSERT INTO documents(id,source_key,filename,created_at) VALUES(?,?,?,?) ON CONFLICT(source_key) DO NOTHING",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(source_key)
    .bind(filename)
    .bind(created_at)
    .execute(&mut *connection)
    .await?;
    let id: String = sqlx::query_scalar("SELECT id FROM documents WHERE source_key=?")
        .bind(source_key)
        .fetch_one(connection)
        .await?;
    Uuid::parse_str(&id).context("El catálogo contiene una identidad documental inválida")
}
