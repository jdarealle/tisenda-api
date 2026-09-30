use super::{SkippedFile, state};
use crate::{
    Chunk, Config,
    chunking::chunk_document,
    documents::{Document, DocumentInventory},
};
use anyhow::{Result, bail};
use qdrant_client::qdrant::PointId;
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

pub(super) struct PendingDocument<'a> {
    pub source: &'a Document,
    pub id: Uuid,
    pub chunks: Vec<Chunk>,
    pub obsolete_ids: Vec<PointId>,
}

pub(super) struct SyncPlan<'a> {
    pub changed: Vec<PendingDocument<'a>>,
    pub files_new: usize,
    pub files_updated: usize,
    pub files_unchanged: usize,
    pub missing: Vec<String>,
    pub preserved: Vec<String>,
    pub skipped: Vec<SkippedFile>,
    pub expected_chunks: usize,
}

pub(super) fn build<'a>(
    config: &Config,
    documents: &'a [Document],
    inventory: &DocumentInventory,
    manifest: &state::Manifest,
    corpus: &Uuid,
    pipeline: &str,
) -> Result<SyncPlan<'a>> {
    let mut plan = SyncPlan {
        changed: Vec::new(),
        files_new: 0,
        files_updated: 0,
        files_unchanged: 0,
        missing: Vec::new(),
        preserved: Vec::new(),
        skipped: Vec::new(),
        expected_chunks: manifest.values().map(Vec::len).sum(),
    };
    let mut valid_paths = BTreeSet::new();
    for document in documents {
        let id = state::document_id(corpus, &document.relative_path);
        let previous = manifest.get(&document.relative_path);
        if previous.is_some_and(|points| state::unchanged(document, &id, pipeline, points)) {
            plan.files_unchanged += 1;
            valid_paths.insert(document.relative_path.clone());
            continue;
        }
        let chunks = chunk_document(document, config);
        if chunks.is_empty() {
            plan.skipped.push(SkippedFile {
                path: document.relative_path.clone(),
                reason: "sin texto fragmentable".into(),
            });
            continue;
        }
        valid_paths.insert(document.relative_path.clone());
        if previous.is_some() {
            plan.files_updated += 1;
        } else {
            plan.files_new += 1;
        }
        let previous = previous.map(Vec::as_slice).unwrap_or_default();
        plan.expected_chunks = plan.expected_chunks - previous.len() + chunks.len();
        let keep: HashSet<PointId> = (0..chunks.len())
            .map(|position| state::chunk_id(&id, position))
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
    if valid_paths.is_empty() {
        bail!(
            "No hay documentos con fragmentos admitidos; se conserva el índice y no se realizan eliminaciones"
        );
    }
    plan.missing = manifest
        .keys()
        .filter(|path| !inventory.contains(path))
        .cloned()
        .collect();
    plan.preserved = manifest
        .keys()
        .filter(|path| inventory.contains(path) && !valid_paths.contains(*path))
        .cloned()
        .collect();
    Ok(plan)
}
