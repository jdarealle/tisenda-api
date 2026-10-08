use super::{
    Selection,
    progress::{PersistenceError, Progress},
    retry,
    source::SourceRoot,
    store::{Job, Store},
    types::{AcceptedBatch, BatchResult, Pagination},
};
use crate::{
    Config, ServerConfig,
    docling::{self, ArchiveLimits, Client, FileReport},
    ingest,
};
use anyhow::Result;
use std::{path::Path, sync::Arc};
use tokio::sync::Notify;

pub(crate) struct Manager {
    config: Config,
    server: ServerConfig,
    client: Client,
    root: Arc<SourceRoot>,
    pub(super) store: Arc<Store>,
    pub(super) wake: Notify,
}
impl Manager {
    pub(crate) async fn new(config: Config, mut server: ServerConfig) -> Result<Self> {
        server.validate(&config)?;
        let root = Arc::new(SourceRoot::new(&server.documents_root)?);
        let client = Client::new(&server)?;
        let store =
            Arc::new(Store::open(&server.ingestions_db_path, &server.ingestions_temp_root).await?);
        Ok(Self {
            config,
            server,
            client,
            root,
            store,
            wake: Notify::new(),
        })
    }
    pub(crate) async fn select(&self, selection: Selection) -> Result<Vec<String>> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || root.select(&selection.paths)).await?
    }
    pub(crate) async fn enqueue(&self, keys: Vec<String>) -> Result<AcceptedBatch> {
        let reports = keys.into_iter().map(|key| self.report(key)).collect();
        let accepted = self.store.enqueue(reports).await.inspect_err(|_| {
            self.store.stop();
        })?;
        self.wake.notify_one();
        Ok(accepted)
    }
    pub(crate) async fn batch(&self, id: &str, page: &Pagination) -> Result<Option<BatchResult>> {
        self.store.batch(id, page).await
    }
    fn report(&self, key: String) -> FileReport {
        let filename = key.rsplit('/').next().unwrap_or(&key).to_owned();
        let profile = self.client.profile(&self.config, &filename);
        FileReport::new(filename, key, profile)
    }
    fn limits(&self) -> ArchiveLimits {
        ArchiveLimits {
            download: self.server.max_download_bytes,
            expanded: self.server.max_expanded_bytes,
            entries: self.server.max_archive_entries,
            file: self.config.max_file_bytes,
        }
    }

    pub(super) async fn execute(&self, mut job: Job) -> Result<()> {
        if job.recover {
            // Reconcile the last observed version before rereading an original that may have changed.
            let progress = Progress::new(self.store.clone(), job.id.clone(), job.attempts);
            progress.stage(&mut job.report, "check_index").await?;
            match ingest::original_unchanged(&self.config, &job.report, &progress).await {
                Ok(Some(chunks)) => {
                    complete_unchanged(&mut job.report, chunks);
                    return self.store.finish(&job, None).await;
                }
                Err(error) if error.is::<PersistenceError>() => return Err(error),
                Err(error) if retry::transient(&error) => {
                    // A failed query is not absence, even after the third interrupted attempt.
                    job.report.fail(&error);
                    return self.store.finish(&job, Some(10)).await;
                }
                Err(error) => {
                    job.report.fail(&error);
                    return self.store.finish(&job, None).await;
                }
                Ok(None) => {}
            }
            if job.attempts >= 3 {
                job.report.fail(&anyhow::anyhow!(
                    "Intentos agotados tras reconciliar la ejecución interrumpida"
                ));
                return self.store.finish(&job, None).await;
            }
        }
        job.report = self.report(job.report.source_key.clone());
        job.report.status = "processing".into();
        self.store.begin_attempt(&mut job).await?;
        let progress = Progress::new(self.store.clone(), job.id.clone(), job.attempts);
        let mut delay = None;
        if docling::input_format(&job.report.filename).is_none() {
            job.report.reject();
        } else if let Err(error) = self.process(&mut job.report, &progress).await {
            if error.is::<PersistenceError>() {
                return Err(error);
            }
            if retry::transient(&error) {
                delay = retry::delay(job.attempts);
            }
            job.report.fail(&error);
            if error.is::<super::source::InvalidSource>() {
                job.report.status = "rejected".into();
                if let Some(error) = &mut job.report.error {
                    error.code = "invalid_source".into();
                }
            }
        }
        tracing::info!(
            event = "ingestion_job_finished",
            job_id = job.id,
            attempt = job.attempts,
            status = job.report.status,
            result = job.report.result,
            retry_seconds = delay
        );
        self.store.finish(&job, delay).await
    }
    async fn process(&self, report: &mut FileReport, progress: &Progress) -> Result<()> {
        let temporary = tempfile::Builder::new()
            .prefix("rag-document-")
            .tempdir_in(&self.store.temp_root)?;
        let result = self.convert(temporary.path(), report, progress).await;
        if let Err(error) = temporary.close() {
            report
                .warnings
                .push(format!("No se pudo limpiar el temporal: {error}"));
            if report.status == "completed" {
                report.status = "completed_with_warnings".into();
            }
        }
        result
    }
    async fn convert(
        &self,
        directory: &Path,
        report: &mut FileReport,
        progress: &Progress,
    ) -> Result<()> {
        progress.stage(report, "snapshot").await?;
        report.original_sha256 = Some(
            self.root
                .snapshot(
                    &report.source_key,
                    &directory.join("original"),
                    self.server.max_original_bytes,
                )
                .await?,
        );
        progress.stage(report, "check_index").await?;
        if let Some(chunks) = ingest::original_unchanged(&self.config, report, progress).await? {
            complete_unchanged(report, chunks);
            return Ok(());
        }
        docling::process_original(
            &self.config,
            &self.client,
            self.limits(),
            directory,
            report,
            progress,
        )
        .await
    }
}
fn complete_unchanged(report: &mut FileReport, chunks: usize) {
    report.chunks = chunks;
    report.result = Some("unchanged".into());
    report.status = if report.warnings.is_empty() {
        "completed"
    } else {
        "completed_with_warnings"
    }
    .into();
    report.stage = "done".into();
    report.error = None;
}
