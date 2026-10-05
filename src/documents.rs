use rig::Embed;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Native Docling JSONL record. `text` includes the structural context for embeddings.
#[derive(Clone, Debug, Deserialize, Serialize, Embed)]
pub struct DoclingChunk {
    pub filename: String,
    pub chunk_index: usize,
    #[embed]
    pub text: String,
    pub raw_text: Option<String>,
    pub num_tokens: Option<usize>,
    pub headings: Option<Vec<String>>,
    pub captions: Option<Vec<String>>,
    pub doc_items: Vec<String>,
    pub page_numbers: Option<Vec<usize>>,
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

pub(crate) struct Document {
    pub(crate) filename: String,
    pub(crate) source_key: String,
    pub(crate) chunks: Vec<DoclingChunk>,
    pub(crate) content_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Embed)]
pub(crate) struct Chunk {
    /// Missing only in the legacy basename-based index, which remains readable.
    #[serde(default)]
    pub(crate) source_key: Option<String>,
    #[embed]
    #[serde(flatten)]
    pub(crate) native: DoclingChunk,
    pub(crate) content_hash: String,
    pub(crate) embedding_version: String,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dimension: usize,
    pub(crate) embedding_preprocessing: String,
}

// Qdrant's gRPC integer type is i64. Keep larger native integers exact as strings
// instead of letting the client convert them to lossy floating-point numbers.
pub(crate) fn prepare_qdrant_metadata(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Number(number)
            if number.as_u64().is_some_and(|n| n > i64::MAX as u64) =>
        {
            *value = number.to_string().into();
        }
        serde_json::Value::Object(fields) => {
            fields.values_mut().for_each(prepare_qdrant_metadata);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(prepare_qdrant_metadata),
        _ => {}
    }
}
