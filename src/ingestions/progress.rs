use super::store::Store;
use crate::docling::FileReport;
use std::sync::Arc;

/// Marks storage errors so the worker cannot mistake them for document failures.
#[derive(Debug)]
pub(super) struct PersistenceError(anyhow::Error);
impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No se pudo persistir el progreso: {}", self.0)
    }
}
impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub(crate) struct Progress {
    store: Arc<Store>,
    job_id: String,
    attempt: i64,
}
impl Progress {
    pub(super) fn new(store: Arc<Store>, job_id: String, attempt: i64) -> Self {
        Self {
            store,
            job_id,
            attempt,
        }
    }
    pub(crate) fn check(&self) -> anyhow::Result<()> {
        self.store.check().map_err(|e| PersistenceError(e).into())
    }
    pub(crate) async fn save(&self, report: &FileReport) -> anyhow::Result<()> {
        self.check()?;
        self.store
            .checkpoint(&self.job_id, self.attempt, report)
            .await
            .map_err(|e| {
                self.store.stop();
                PersistenceError(e).into()
            })
    }
    pub(crate) async fn stage(&self, report: &mut FileReport, stage: &str) -> anyhow::Result<()> {
        tracing::debug!(event = "document_stage", stage);
        report.stage = stage.into();
        report.status = "processing".into();
        self.save(report).await
    }
}
