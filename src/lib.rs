mod answer;
mod chunking;
mod config;
mod documents;
mod embeddings;
mod index;
mod ingest;

pub use answer::{Answer, AnswerRequest, Source, answer};
pub use config::Config;
pub use ingest::{IngestOptions, IngestReport, SkippedFile, ingest};

use rig::Embed;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Embed)]
pub(crate) struct Chunk {
    #[embed]
    pub text: String,
    pub relative_path: String,
    pub section: Option<String>,
    pub chunk_index: usize,
    pub content_hash: String,
    pub embedding_version: String,
    pub embedding_model: String,
    pub embedding_dimension: usize,
    pub embedding_preprocessing: String,
}
