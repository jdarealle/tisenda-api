use crate::docling::FileReport;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Archivos o carpetas relativos a DOCUMENTS_ROOT; omitido o vacío selecciona toda la raíz.
    #[serde(default)]
    pub(super) paths: Vec<String>,
}

/// Contadores de estados excluyentes de los documentos del lote.
#[derive(Default, Serialize, ToSchema)]
struct Counts {
    total: usize,
    completed: usize,
    completed_with_warnings: usize,
    rejected: usize,
    failed: usize,
}

/// Resultado final del lote; un HTTP 200 puede incluir documentos rechazados o fallidos.
#[derive(Serialize, ToSchema)]
pub(crate) struct BatchResult {
    counts: Counts,
    pub(super) documents: Vec<FileReport>,
}

impl BatchResult {
    pub(super) fn new(documents: Vec<FileReport>) -> Self {
        let mut counts = Counts::default();
        for report in &documents {
            counts.total += 1;
            match report.status.as_str() {
                "completed" => counts.completed += 1,
                "completed_with_warnings" => counts.completed_with_warnings += 1,
                "rejected" => counts.rejected += 1,
                _ => counts.failed += 1,
            }
        }
        Self { counts, documents }
    }
}
