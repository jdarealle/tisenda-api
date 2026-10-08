//! API de ingesta de documentos convertidos por Docling.

mod document_processor;
mod index_state;
mod pipeline;
mod plan;
mod preflight;
mod types;
mod write;

pub use pipeline::ingest_documents;
pub(crate) use pipeline::ingest_with_progress;
pub(crate) use preflight::original_unchanged;
pub use types::{DocumentIngestResult, IngestDocument, IngestReport};

pub(crate) use document_processor::{validate_chunks, validate_filename, validate_source_key};
