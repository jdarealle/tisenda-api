use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct ArchiveLimits {
    pub(crate) download: u64,
    pub(crate) expanded: u64,
    pub(crate) file: u64,
    pub(crate) entries: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FileError {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FileReport {
    pub(crate) filename: String,
    pub(crate) source_key: String,
    pub(crate) status: String,
    pub(crate) stage: String,
    pub(crate) original_sha256: Option<String>,
    pub(crate) task_ids: Vec<String>,
    pub(crate) chunks: usize,
    pub(crate) warnings: Vec<String>,
    pub(crate) error: Option<FileError>,
    pub(crate) profile: Value,
}

impl FileReport {
    pub(crate) fn new(filename: String, source_key: String, profile: Value) -> Self {
        Self {
            filename,
            source_key,
            status: "pending".into(),
            stage: "validation".into(),
            original_sha256: None,
            task_ids: vec![],
            chunks: 0,
            warnings: vec![],
            error: None,
            profile,
        }
    }

    pub(super) fn stage(&mut self, stage: &str) {
        self.stage = stage.into();
        self.status = "processing".into();
    }

    pub(crate) fn reject(&mut self) {
        self.status = "rejected".into();
        self.error = Some(FileError {
            code: "unsupported_extension".into(),
            message: "Extensión no admitida por la política documental de la API".into(),
        });
    }

    pub(crate) fn fail(&mut self, error: &anyhow::Error) {
        self.status = "failed".into();
        self.error = Some(FileError {
            code: "processing_failed".into(),
            message: format!("{error:#}"),
        });
        tracing::error!(document = %self.filename, source_key = %self.source_key, stage = %self.stage, error = %format!("{error:#}"), "ingesta_fallida");
    }
}
