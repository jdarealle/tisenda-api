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

/// A converted document. Its source key is relative to the configured root and identifies it within the corpus.
#[derive(Clone, Debug)]
pub struct IngestDocument {
    pub filename: String,
    pub source_key: String,
    pub chunks: Vec<DoclingChunk>,
}

#[derive(Debug)]
pub struct DocumentIngestResult {
    pub filename: String,
    pub source_key: String,
    pub result: Result<IngestReport>,
}
