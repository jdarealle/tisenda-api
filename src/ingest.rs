use crate::{
    Config, chunking::chunk_document, documents::read_documents, embeddings::TeiModel, index,
};
use anyhow::{Context, Result, bail};
use rig::{embeddings::EmbeddingsBuilder, vector_store::InsertDocuments};
use serde::Serialize;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct IngestOptions {
    pub source_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IngestReport {
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub skipped: Vec<SkippedFile>,
    pub chunks: usize,
    pub active_collection: String,
}

pub async fn ingest(config: &Config, options: IngestOptions) -> Result<IngestReport> {
    let (documents, skipped, files_seen) =
        read_documents(&options.source_dir, config.max_file_bytes)?;
    let mut chunks = Vec::new();
    for document in &documents {
        chunks.extend(chunk_document(document, config));
    }
    if chunks.is_empty() {
        bail!("No hay fragmentos admitidos para indexar; se conserva el índice anterior");
    }
    let model = TeiModel::new(config)?;
    model
        .preflight()
        .await
        .context("TEI no está listo para la ingesta")?;
    let client = index::client(config)?;
    let old = index::active_collection(&client, &config.qdrant_alias).await?;
    let collection = format!(
        "rag_{}_{}",
        config.embedding_version(),
        Uuid::new_v4().simple()
    );
    index::create_collection(&client, config, &collection).await?;
    let store = index::store(client.clone(), model.clone(), &collection);
    for batch in chunks.chunks(16) {
        let embedded = EmbeddingsBuilder::new(model.clone())
            .documents(batch.to_vec())?
            .build()
            .await
            .context("Falló la generación de embeddings; el alias anterior sigue activo")?;
        store
            .insert_documents(embedded)
            .await
            .context("Falló la inserción en Qdrant; el alias anterior sigue activo")?;
    }
    index::verify_count(&client, &collection, chunks.len())
        .await
        .context("La colección nueva está incompleta; el alias anterior sigue activo")?;
    index::publish_alias(config, old.as_deref(), &collection)
        .await
        .context("No se pudo activar la colección nueva; el alias anterior sigue activo")?;
    if index::active_collection(&client, &config.qdrant_alias)
        .await?
        .as_deref()
        != Some(collection.as_str())
    {
        bail!("Qdrant no confirmó la colección nueva como índice activo");
    }
    Ok(IngestReport {
        files_seen,
        files_indexed: documents.len(),
        files_skipped: skipped.len(),
        skipped,
        chunks: chunks.len(),
        active_collection: collection,
    })
}
