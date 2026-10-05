use super::index_state;
use crate::{
    Config,
    documents::{Chunk, Document},
};
use qdrant_client::qdrant::PointId;
use std::collections::HashSet;
use uuid::Uuid;

pub(super) struct PendingDocument<'a> {
    pub(super) source: &'a Document,
    pub(super) id: Uuid,
    pub(super) chunks: Vec<Chunk>,
    pub(super) obsolete_ids: Vec<PointId>,
}

pub(super) struct SyncPlan<'a> {
    pub(super) changed: Vec<PendingDocument<'a>>,
    pub(super) files_new: usize,
    pub(super) files_updated: usize,
    pub(super) files_unchanged: usize,
    pub(super) expected_chunks: usize,
}

pub(super) fn build<'a>(
    config: &Config,
    documents: &'a [Document],
    manifest: &index_state::Manifest,
    corpus: &Uuid,
    pipeline: &str,
) -> SyncPlan<'a> {
    let mut plan = SyncPlan {
        changed: Vec::new(),
        files_new: 0,
        files_updated: 0,
        files_unchanged: 0,
        expected_chunks: manifest.values().map(Vec::len).sum(),
    };
    for document in documents {
        let id = index_state::document_id(corpus, &document.source_key);
        let previous = manifest.get(&document.source_key);
        if previous.is_some_and(|points| index_state::unchanged(document, &id, pipeline, points)) {
            plan.files_unchanged += 1;
            continue;
        }
        let chunks: Vec<_> = document
            .chunks
            .iter()
            .cloned()
            .map(|native| Chunk {
                native,
                source_key: Some(document.source_key.clone()),
                content_hash: document.content_hash.clone(),
                embedding_version: config.embedding_version(),
                embedding_model: config.embedding_model.clone(),
                embedding_dimension: config.embedding_dimension,
                embedding_preprocessing: crate::config::EMBEDDING_PREPROCESSING.into(),
            })
            .collect();
        if previous.is_some() {
            plan.files_updated += 1;
        } else {
            plan.files_new += 1;
        }
        let previous = previous.map(Vec::as_slice).unwrap_or_default();
        plan.expected_chunks = plan.expected_chunks - previous.len() + chunks.len();
        let keep: HashSet<PointId> = (0..chunks.len())
            .map(|position| index_state::chunk_id(&id, position))
            .collect();
        let obsolete_ids = previous
            .iter()
            .filter(|point| !keep.contains(&point.id))
            .map(|point| point.id.clone())
            .collect();
        plan.changed.push(PendingDocument {
            source: document,
            id,
            chunks,
            obsolete_ids,
        });
    }
    plan
}
