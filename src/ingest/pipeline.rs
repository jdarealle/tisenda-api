use super::{
    DocumentIngestResult, IngestDocument, IngestReport, document_processor, index_state, plan,
    write,
};
use crate::{Config, documents::Document, qdrant, tei::TeiModel};
use anyhow::{Context, Result, bail};

/// Upsert supplied documents independently, without treating absent documents as deleted.
pub async fn ingest_documents(
    config: &Config,
    documents: Vec<IngestDocument>,
) -> Vec<DocumentIngestResult> {
    ingest_with_progress(config, documents, None).await
}

pub(crate) async fn ingest_with_progress(
    config: &Config,
    documents: Vec<IngestDocument>,
    progress: Option<&crate::ingestions::Progress>,
) -> Vec<DocumentIngestResult> {
    let mut results = Vec::with_capacity(documents.len());
    let mut names = std::collections::HashMap::new();
    let mut ids = std::collections::HashMap::new();
    for doc in &documents {
        *names.entry(doc.source_key.clone()).or_insert(0usize) += 1;
        *ids.entry(doc.document_id).or_insert(0usize) += 1;
    }
    for doc in documents {
        let filename = doc.filename.clone();
        let source_key = doc.source_key.clone();
        let document_id = doc.document_id;
        let result = async {
            document_processor::validate_filename(&filename)?;
            if names[&source_key] != 1 {
                bail!("source_key duplicada");
            }
            if ids[&document_id] != 1 {
                bail!("document_id duplicado");
            }
            let document = document_processor::prepare(config, doc)?;
            ingest_document(config, document, progress).await
        }
        .await;
        results.push(DocumentIngestResult {
            document_id,
            filename,
            source_key,
            result,
        });
    }
    results
}

#[tracing::instrument(skip_all, level = "debug", name = "index_document")]
async fn ingest_document(
    config: &Config,
    document: Document,
    progress: Option<&crate::ingestions::Progress>,
) -> Result<IngestReport> {
    if let Some(progress) = progress {
        progress.check()?;
    }
    let _writer = index_state::writer_lock(config)?;
    let model = TeiModel::new(config)?;
    // Info identifica el modelo; una ejecución sin cambios no necesita Embed.
    let identity = model
        .check_model()
        .await
        .context("TEI no está listo para la ingesta")?;
    let client = qdrant::client(config)?;
    let active = qdrant::active_collection(&client, &config.qdrant_alias).await?;
    let collection = active
        .clone()
        .unwrap_or_else(|| index_state::stable_collection(config));
    let exists = crate::logging::operation("qdrant", "collection_exists", async {
        Ok(client.collection_exists(collection.as_str()).await?)
    })
    .await?;
    let binding = if exists {
        qdrant::collection_binding(&client, config, &collection).await?
    } else {
        qdrant::CollectionBinding::new(config, identity.clone())
    };
    binding.validate_model(config, &identity)?;
    binding.validate_source(config)?;
    binding.validate_ingestion()?;
    let manifest = if exists {
        index_state::read_manifest(&client, config, &collection).await?
    } else {
        index_state::Manifest::new()
    };
    index_state::ensure_source_identity(
        document.document_id,
        &document.source_key,
        manifest.values().flatten(),
    )?;
    let pipeline = index_state::pipeline_version(config, &identity);
    let plan = plan::build(
        config,
        std::slice::from_ref(&document),
        &manifest,
        &pipeline,
    );
    if !plan.changed.is_empty() {
        let limit = model.input_limit().await?;
        let counts = model.chunk_counts(&document.chunks, limit).await?;
        for (index, count) in counts.into_iter().enumerate() {
            if count.is_none_or(|n| n == 0 || n > limit) {
                bail!(
                    "Fragmento {index} excede la capacidad real de TEI ({limit} tokens): {count:?}"
                );
            }
        }
        if let Some(progress) = progress {
            progress.check()?;
        }
        let checked_identity = model
            .preflight()
            .await
            .context("TEI no está listo para generar embeddings")?;
        if checked_identity != identity {
            bail!(
                "TEI cambió de revisión durante la preparación de la ingesta; no se modifica el índice"
            );
        }
    }
    write::ensure_alias(&client, config, active.as_deref()).await?;
    if let Some(progress) = progress {
        progress.check()?;
    }
    if !exists {
        qdrant::create_collection(&client, config, &collection, &binding).await?;
    }
    qdrant::ensure_document_indexes(&client, &collection).await?;
    let chunks_written = write::documents(
        &client,
        &model,
        &collection,
        &pipeline,
        &plan.changed,
        progress,
    )
    .await?;
    let expected = plan.expected_chunks;
    // Confirmar los IDs y metadatos nuevos mientras los sobrantes siguen presentes.
    if !plan.changed.is_empty() {
        let confirmed = index_state::read_manifest(&client, config, &collection).await?;
        for document in &plan.changed {
            if !confirmed.get(&document.id).is_some_and(|points| {
                index_state::written(
                    document.source,
                    &document.id,
                    &pipeline,
                    document.chunks.len(),
                    points,
                )
            }) {
                bail!(
                    "Qdrant no confirmó todos los fragmentos de {}; se conservan los puntos sobrantes",
                    document.source.filename
                );
            }
        }
    }
    // Todas las escrituras terminaron antes de retirar cualquier punto.
    write::ensure_alias(&client, config, active.as_deref()).await?;
    for document in &plan.changed {
        if document.obsolete_ids.is_empty() {
            continue;
        }
        if let Some(progress) = progress {
            progress.check()?;
        }
        qdrant::delete_points(&client, &collection, &document.obsolete_ids).await?;
    }
    qdrant::verify_count(&client, &collection, expected)
        .await
        .context("El índice no tiene el conteo esperado")?;
    if active.is_none() {
        write::ensure_alias(&client, config, None).await?;
        if let Some(progress) = progress {
            progress.check()?;
        }
        qdrant::publish_alias(&client, &config.qdrant_alias, &collection).await?;
    }
    write::ensure_alias(&client, config, Some(collection.as_str())).await?;
    tracing::debug!(
        event = "index_document_completed",
        document_id = %document.document_id,
        chunks_written,
        files_new = plan.files_new,
        files_updated = plan.files_updated,
        files_unchanged = plan.files_unchanged
    );
    Ok(IngestReport {
        chunks: expected,
        active_collection: collection,
        files_new: plan.files_new,
        files_updated: plan.files_updated,
        files_unchanged: plan.files_unchanged,
        chunks_written,
    })
}
