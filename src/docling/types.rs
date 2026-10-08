use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Clone, Debug)]
pub(crate) struct ArchiveLimits {
    pub(crate) download: u64,
    pub(crate) expanded: u64,
    pub(crate) file: u64,
    pub(crate) entries: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct FileError {
    /// unsupported_extension, invalid_source o processing_failed.
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct FileReport {
    pub(crate) filename: String,
    pub(crate) source_key: String,
    /// pending, processing, completed, completed_with_warnings, rejected o failed.
    pub(crate) status: String,
    /// Etapa: validation, snapshot, check_index, conversion, download, tokens, rechunk, index o done.
    pub(crate) stage: String,
    #[schema(required = true)]
    pub(crate) original_sha256: Option<String>,
    pub(crate) task_ids: Vec<String>,
    /// indexed o unchanged al terminar correctamente.
    #[serde(default)]
    pub(crate) result: Option<String>,
    /// Fragmentos preparados; solo un estado completed o completed_with_warnings confirma su indexación.
    pub(crate) chunks: usize,
    pub(crate) warnings: Vec<String>,
    #[schema(required = true)]
    pub(crate) error: Option<FileError>,
    /// Perfil de procesamiento expresado como JSON libre.
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
            result: None,
            chunks: 0,
            warnings: vec![],
            error: None,
            profile,
        }
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
    }
}
