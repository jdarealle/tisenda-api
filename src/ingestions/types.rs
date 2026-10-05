use crate::docling::FileReport;
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    #[serde(default)]
    pub(super) paths: Vec<String>,
}

#[derive(Default, Serialize)]
struct Counts {
    total: usize,
    completed: usize,
    completed_with_warnings: usize,
    rejected: usize,
    failed: usize,
}

#[derive(Serialize)]
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
