use super::{BatchResult, Selection, source::SourceRoot};
use crate::{
    Config, ServerConfig,
    docling::{self, ArchiveLimits, Client, FileReport},
    qdrant,
};
use anyhow::{Context, Result};
use std::{path::Path, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
        tokio::task::spawn_blocking(move || root.select(&selection.paths)).await?
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
    pub(crate) async fn run(&self, keys: Vec<String>) -> BatchResult {
        let mut documents = Vec::with_capacity(keys.len());
        for key in keys {
            // select() has already validated UTF-8, relative paths and basenames.
            let filename = key.rsplit('/').next().unwrap_or(&key).to_owned();
            let mut report = FileReport::new(filename, key, self.client.profile(&self.config));
            if docling::input_format(&report.filename).is_none() {
                report.reject();
            } else if let Err(error) = self.process(&mut report).await {
                report.fail(&error);
            }
            documents.push(report);
        }
        BatchResult::new(documents)
    }

    async fn process(&self, report: &mut FileReport) -> Result<()> {
        // TempDir removes the original and canonical JSON on success, error or
        // future cancellation. No exported ZIP or execution manifest is saved.
        let temporary = tempfile::Builder::new().prefix("rag-document-").tempdir()?;
        let result = self.convert(temporary.path(), report).await;
        if let Err(error) = temporary.close() {
            tracing::warn!(source_key = %report.source_key, %error, "No se pudo limpiar el temporal");
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
