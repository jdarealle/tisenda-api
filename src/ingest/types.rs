use crate::DoclingChunk;
use anyhow::Result;
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct IngestReport {
    pub chunks: usize,
    pub active_collection: String,
    pub files_new: usize,
    pub files_updated: usize,
    pub files_unchanged: usize,
    pub chunks_written: usize,
}

/// A converted document with a persistent, caller-assigned identity.
/// Reuse the same ID on retries and updates; source_key is only a relative locator.
#[derive(Clone, Debug)]
pub struct IngestDocument {
    pub document_id: Uuid,
    pub filename: String,
    pub source_key: String,
    pub chunks: Vec<DoclingChunk>,
}

#[derive(Debug)]
pub struct DocumentIngestResult {
    pub document_id: Uuid,
    pub filename: String,
    pub source_key: String,
    pub result: Result<IngestReport>,
}
