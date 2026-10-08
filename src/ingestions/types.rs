use crate::docling::FileReport;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Rutas relativas; omitido o vacío selecciona toda DOCUMENTS_ROOT.
    #[serde(default)]
    pub(super) paths: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AcceptedBatch {
    pub(crate) batch_id: String,
    pub(crate) total: usize,
    pub(crate) status_url: String,
}

#[derive(Default, Serialize, ToSchema)]
pub(super) struct Counts {
    pub(super) total: i64,
    pub(super) pending: i64,
    pub(super) processing: i64,
    pub(super) completed: i64,
    pub(super) completed_with_warnings: i64,
    pub(super) rejected: i64,
    pub(super) failed: i64,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct BatchResult {
    pub(super) batch_id: String,
    /// pending, processing o completed; revisar counts para conocer los fallos.
    pub(super) status: String,
    pub(super) counts: Counts,
    /// Fechas UTC expresadas en milisegundos desde Unix epoch.
    pub(super) created_at: i64,
    pub(super) finished_at: Option<i64>,
    pub(super) limit: i64,
    pub(super) offset: i64,
    pub(super) documents: Vec<JobReport>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct JobReport {
    pub(super) job_id: String,
    pub(super) attempts: i64,
    pub(super) created_at: i64,
    pub(super) started_at: Option<i64>,
    pub(super) updated_at: i64,
    pub(super) finished_at: Option<i64>,
    pub(super) next_attempt_at: Option<i64>,
    #[serde(flatten)]
    pub(super) report: FileReport,
}

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pagination {
    /// Tamaño de página entre 1 y 500; por defecto 100.
    #[serde(default = "default_limit")]
    pub(crate) limit: i64,
    #[serde(default)]
    pub(crate) offset: i64,
}
fn default_limit() -> i64 {
    100
}
impl Pagination {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=500).contains(&self.limit) && self.offset >= 0,
            "Paginación inválida"
        );
        Ok(())
    }
}
