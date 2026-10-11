use super::types::{AcceptedBatch, BatchResult, Counts, JobReport, Pagination};
use crate::docling::FileReport;
use anyhow::{Context, Result, ensure};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) struct Store {
    pool: SqlitePool,
    failed: AtomicBool,
    // Held until every user of the pool has stopped. Never unlink the lock inode.
    _owner: File,
    pub(super) temp_root: PathBuf,
}

pub(super) struct Job {
    pub(super) id: String,
    pub(super) attempts: i64,
    pub(super) recover: bool,
    pub(super) report: FileReport,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_millis() as i64
}

impl Store {
    pub(super) fn stop(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }
    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            !self.failed.load(Ordering::SeqCst),
            "Persistencia de ingesta detenida tras un error"
        );
        Ok(())
    }
    pub(super) async fn open(path: &Path, temp_root: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        tokio::fs::create_dir_all(parent).await?;
        let path = parent
            .canonicalize()?
            .join(path.file_name().context("INGESTIONS_DB_PATH inválida")?);
        ensure!(
            !path.is_symlink(),
            "SQLite no puede ser un enlace simbólico"
        );
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        owner
            .try_lock()
            .context("Otra instancia posee esta cola de ingesta")?;
        // A namespace per database prevents one queue from cleaning another's temporaries.
        let namespace = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            path.as_os_str().as_encoded_bytes(),
        );
        let temp_root = temp_root.join(format!("queue-{namespace}"));
        tokio::fs::create_dir_all(&temp_root).await?;
        ensure!(
            !temp_root.is_symlink(),
            "El directorio temporal no puede ser un enlace"
        );
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            if matches!(error, sqlx::migrate::MigrateError::VersionMismatch(_)) {
                tracing::error!(
                    event = "catalog_schema_incompatible",
                    "SQLite usa un esquema anterior: reinicializa el prototipo según el README; conserva los originales"
                );
                return Err(error).context("SQLite usa un esquema anterior al catálogo documental. Reinicializa el prototipo según el README; no se borran datos automáticamente");
            }
            return Err(error.into());
        }
        // Only clean interrupted attempts once the database schema is accepted.
        let mut entries = tokio::fs::read_dir(&temp_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("rag-document-")
                && entry.file_type().await?.is_dir()
            {
                tokio::fs::remove_dir_all(entry.path()).await?;
            }
        }
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&pool)
            .await?;
        tracing::info!(event = "ingestion_store_opened", sqlite_version = version);
        let store = Self {
            pool,
            failed: AtomicBool::new(false),
            _owner: owner,
            temp_root,
        };
        let mut tx = store.pool.begin().await?;
        sqlx::query(
            "UPDATE attempts SET outcome='interrupted', finished_at=? WHERE finished_at IS NULL",
        )
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE jobs SET status='pending', recover=1, available_at=?, updated_at=?, report=json_set(report,'$.status','pending') WHERE status='processing'")
            .bind(now()).bind(now()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(store)
    }

    pub(super) async fn enqueue(
        &self,
        keys: Vec<String>,
        make_report: impl Fn(uuid::Uuid, String) -> FileReport,
    ) -> Result<AcceptedBatch> {
        self.check()?;
        let id = uuid::Uuid::now_v7().to_string();
        let timestamp = now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO batches(id,created_at,finished_at) VALUES(?,?,?)")
            .bind(&id)
            .bind(timestamp)
            .bind(keys.is_empty().then_some(timestamp))
            .execute(&mut *tx)
            .await?;
        let total = keys.len();
        for (ordinal, key) in keys.into_iter().enumerate() {
            let document_id = super::catalog::resolve(&mut tx, &key, timestamp).await?;
            let report = make_report(document_id, key.clone());
            ensure!(
                report.document_id == document_id && report.source_key == key,
                "El informe no corresponde al documento registrado"
            );
            sqlx::query("INSERT INTO jobs(id,batch_id,document_id,ordinal,source_key,status,report,available_at,created_at,updated_at) VALUES(?,?,?,?,?,'pending',?,?,?,?)")
                .bind(uuid::Uuid::now_v7().to_string()).bind(&id).bind(document_id.to_string()).bind(ordinal as i64).bind(&key)
                .bind(serde_json::to_string(&report)?).bind(timestamp).bind(timestamp).bind(timestamp).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(AcceptedBatch {
            status_url: format!("/ingestions/{id}"),
            batch_id: id,
            total,
        })
    }

    pub(super) async fn claim(&self) -> Result<Option<Job>> {
        self.check()?;
        let timestamp = now();
        // One atomic write obtains the oldest runnable job, without a read/write upgrade race.
        let row = sqlx::query("UPDATE jobs SET status='processing', updated_at=?, report=json_set(report,'$.status','processing') WHERE sequence=(SELECT sequence FROM jobs WHERE status='pending' AND available_at<=? ORDER BY sequence LIMIT 1) RETURNING id,attempts,recover,report")
            .bind(timestamp).bind(timestamp).fetch_optional(&self.pool).await?;
        row.map(|r| {
            Ok(Job {
                id: r.try_get("id")?,
                attempts: r.try_get("attempts")?,
                recover: r.try_get::<i64, _>("recover")? != 0,
                report: serde_json::from_str(r.try_get("report")?)?,
            })
        })
        .transpose()
    }

    pub(super) async fn begin_attempt(&self, job: &mut Job) -> Result<()> {
        ensure!(job.attempts < 3, "Se agotaron los intentos");
        let timestamp = now();
        let report = serde_json::to_string(&job.report)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE jobs SET attempts=attempts+1, recover=0, started_at=coalesce(started_at,?), updated_at=?, report=? WHERE id=?")
            .bind(timestamp).bind(timestamp).bind(&report).bind(&job.id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO attempts(job_id,number,started_at,outcome,report) VALUES(?,?,?,'processing',?)")
            .bind(&job.id).bind(job.attempts+1).bind(timestamp).bind(report).execute(&mut *tx).await?;
        tx.commit().await?;
        job.attempts += 1;
        job.recover = false;
        Ok(())
    }

    pub(super) async fn checkpoint(
        &self,
        id: &str,
        attempt: i64,
        report: &FileReport,
    ) -> Result<()> {
        let json = serde_json::to_string(report)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE jobs SET report=?,updated_at=? WHERE id=?")
            .bind(&json)
            .bind(now())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE attempts SET report=? WHERE job_id=? AND number=? AND finished_at IS NULL",
        )
        .bind(json)
        .bind(id)
        .bind(attempt)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn finish(&self, job: &Job, delay_secs: Option<i64>) -> Result<()> {
        let timestamp = now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE attempts SET finished_at=?,outcome=?,report=? WHERE job_id=? AND number=? AND finished_at IS NULL")
            .bind(timestamp).bind(&job.report.status).bind(serde_json::to_string(&job.report)?).bind(&job.id).bind(job.attempts).execute(&mut *tx).await?;
        let mut report = job.report.clone();
        if delay_secs.is_some() {
            report.status = "pending".into();
        }
        sqlx::query("UPDATE jobs SET status=?,report=?,available_at=?,updated_at=?,finished_at=? WHERE id=?")
            .bind(&report.status).bind(serde_json::to_string(&report)?).bind(timestamp+delay_secs.unwrap_or(0)*1000).bind(timestamp)
            .bind(delay_secs.is_none().then_some(timestamp)).bind(&job.id).execute(&mut *tx).await?;
        sqlx::query("UPDATE batches SET finished_at=? WHERE id=(SELECT batch_id FROM jobs WHERE id=?) AND NOT EXISTS(SELECT 1 FROM jobs WHERE batch_id=batches.id AND status IN ('pending','processing'))")
            .bind(timestamp).bind(&job.id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn batch(&self, id: &str, page: &Pagination) -> Result<Option<BatchResult>> {
        page.validate()?;
        let mut tx = self.pool.begin().await?;
        let Some(batch) = sqlx::query("SELECT created_at,finished_at FROM batches WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(None);
        };
        let mut counts = Counts::default();
        for row in
            sqlx::query("SELECT status,count(*) AS n FROM jobs WHERE batch_id=? GROUP BY status")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?
        {
            let n: i64 = row.try_get("n")?;
            counts.total += n;
            match row.try_get::<&str, _>("status")? {
                "pending" => counts.pending = n,
                "processing" => counts.processing = n,
                "completed" => counts.completed = n,
                "completed_with_warnings" => counts.completed_with_warnings = n,
                "rejected" => counts.rejected = n,
                "failed" => counts.failed = n,
                _ => unreachable!(),
            }
        }
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE batch_id=? ORDER BY ordinal LIMIT ? OFFSET ?")
                .bind(id)
                .bind(page.limit)
                .bind(page.offset)
                .fetch_all(&mut *tx)
                .await?;
        let documents = rows
            .into_iter()
            .map(|r| {
                Ok(JobReport {
                    job_id: r.try_get("id")?,
                    attempts: r.try_get("attempts")?,
                    created_at: r.try_get("created_at")?,
                    started_at: r.try_get("started_at")?,
                    updated_at: r.try_get("updated_at")?,
                    finished_at: r.try_get("finished_at")?,
                    next_attempt_at: if r.try_get::<&str, _>("status")? == "pending" {
                        Some(r.try_get("available_at")?)
                    } else {
                        None
                    },
                    report: serde_json::from_str(r.try_get("report")?)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        let finished_at: Option<i64> = batch.try_get("finished_at")?;
        Ok(Some(BatchResult {
            batch_id: id.into(),
            status: if finished_at.is_some() {
                "completed"
            } else if counts.processing > 0 || counts.total != counts.pending {
                "processing"
            } else {
                "pending"
            }
            .into(),
            counts,
            created_at: batch.try_get("created_at")?,
            finished_at,
            documents,
            limit: page.limit,
            offset: page.offset,
        }))
    }
}
