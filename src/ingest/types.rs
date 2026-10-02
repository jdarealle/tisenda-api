use crate::DoclingChunk;
use anyhow::Result;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct IngestReport {
    pub chunks: usize,
    pub active_collection: String,
    pub files_new: usize,
    pub files_updated: usize,
    pub files_unchanged: usize,
    pub chunks_written: usize,
}

/// A converted document. Its original filename is its stable identity within the corpus.
#[derive(Clone, Debug)]
pub struct IngestDocument {
    pub filename: String,
    pub chunks: Vec<DoclingChunk>,
}

#[derive(Debug)]
pub struct DocumentIngestResult {
    pub filename: String,
    pub result: Result<IngestReport>,
}
