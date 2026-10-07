use super::{BatchResult, Selection, source::SourceRoot};
use crate::logging::Failure;
use crate::{
    Config, ServerConfig,
    docling::{self, ArchiveLimits, Client, FileReport},
    qdrant,
};
use anyhow::{Context, Result};
use std::{path::Path, sync::Arc, time::Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::Instrument;

pub(crate) struct Manager {
    config: Config,
    server: ServerConfig,
    client: Client,
    root: Arc<SourceRoot>,
    slots: Arc<Semaphore>,
}

impl Manager {
    pub(crate) fn new(config: Config, mut server: ServerConfig) -> Result<Self> {
        server.validate(&config)?;
        Ok(Self {
            client: Client::new(&server)?,
            root: Arc::new(SourceRoot::new(&server.documents_root)?),
            config,
            server,
            slots: Arc::new(Semaphore::new(1)),
        })
    }

    pub(crate) fn reserve(&self) -> Result<OwnedSemaphorePermit> {
        self.slots
            .clone()
            .try_acquire_owned()
            .context("Ya existe un lote activo")
    }

    pub(crate) async fn select(&self, selection: Selection) -> Result<Vec<String>> {
        let root = self.root.clone();
        let span = tracing::Span::current();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        tokio::task::spawn_blocking(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                span.in_scope(|| {
                    let result = root.select(&selection.paths);
                    tracing::debug!(
                        event = "document_selection_completed",
                        documents = result.as_ref().ok().map(Vec::len)
                    );
                    result
                })
            })
        })
        .await?
    }

    pub(crate) async fn check_index(&self) -> Result<Option<String>> {
        let client = qdrant::client(&self.config)?;
        if let Some(active) = qdrant::active_collection(&client, &self.config.qdrant_alias).await? {
            let binding = qdrant::collection_binding(&client, &self.config, &active).await?;
            binding.validate_source(&self.config)?;
            return Ok(binding
                .validate_ingestion()
                .err()
                .map(|error| error.to_string()));
        }
        Ok(None)
    }

    fn limits(&self) -> ArchiveLimits {
        ArchiveLimits {
            download: self.server.max_download_bytes,
            expanded: self.server.max_expanded_bytes,
            entries: self.server.max_archive_entries,
            file: self.config.max_file_bytes,
        }
    }

    /// Synchronous batch operation; no detached jobs or durable task state.
    #[tracing::instrument(skip_all, name = "ingestion_batch", fields(batch_id = %uuid::Uuid::new_v4()))]
    pub(crate) async fn run(&self, keys: Vec<String>) -> BatchResult {
        let started = Instant::now();
        tracing::info!(event = "ingestion_batch_started", documents = keys.len());
        let mut documents = Vec::with_capacity(keys.len());
        for key in keys {
            let report = async {
                let started = Instant::now();
                // select() has already validated relative paths and basenames.
                let filename = key.rsplit('/').next().unwrap_or(&key).to_owned();
                let profile = self.client.profile(&self.config, &filename);
                let mut report = FileReport::new(filename, key, profile);
                let failure = if docling::input_format(&report.filename).is_none() {
                    report.reject();
                    Some(Failure::new("unsupported_extension"))
                } else if let Err(error) = self.process(&mut report).await {
                    report.fail(&error);
                    Some(Failure::from_error("processing_failed", &error))
                } else {
                    None
                };
                log_document(&report, failure.as_ref(), started);
                report
            }
            .instrument(tracing::info_span!("document", document_id = %uuid::Uuid::new_v4()))
            .await;
            documents.push(report);
        }
        let result = BatchResult::new(documents);
        result.log_summary(started.elapsed());
        result
    }

    async fn process(&self, report: &mut FileReport) -> Result<()> {
        // TempDir removes the original and canonical JSON on success, error or
        // future cancellation. No exported ZIP or execution manifest is saved.
        let temporary = tempfile::Builder::new().prefix("rag-document-").tempdir()?;
        let result = self.convert(temporary.path(), report).await;
        if let Err(error) = temporary.close() {
            tracing::warn!(event = "temporary_cleanup_failed", error_kind = "io");
            report
                .warnings
                .push(format!("No se pudo limpiar el temporal: {error}"));
            if report.status == "completed" {
                report.status = "completed_with_warnings".into();
            }
        }
        result
    }

    async fn convert(&self, directory: &Path, report: &mut FileReport) -> Result<()> {
        tracing::debug!(event = "document_stage", stage = "snapshot");
        report.stage = "snapshot".into();
        report.original_sha256 = Some(
            self.root
                .snapshot(
                    &report.source_key,
                    &directory.join("original"),
                    self.server.max_original_bytes,
                )
                .await?,
        );
        docling::process_original(&self.config, &self.client, self.limits(), directory, report)
            .await
    }
}

fn log_document(report: &FileReport, failure: Option<&Failure>, started: Instant) {
    macro_rules! completed {
        ($level:ident) => {
            tracing::$level!(
                event = "document_completed", status = %report.status, stage = %report.stage,
                chunks = report.chunks, warnings = report.warnings.len(),
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                error_code = failure.map(|failure| failure.code),
                error_kind = failure.map(|failure| failure.kind),
                upstream_http_status = failure.and_then(|failure| failure.http_status),
                grpc_code = failure.and_then(|failure| failure.grpc_code),
            )
        };
    }
    match report.status.as_str() {
        "failed" => completed!(error),
        "rejected" | "completed_with_warnings" => completed!(warn),
        _ => completed!(info),
    }
}
