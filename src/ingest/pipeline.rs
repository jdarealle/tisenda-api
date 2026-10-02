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
    let mut results = Vec::with_capacity(documents.len());
    let mut names = std::collections::HashMap::new();
    for doc in &documents {
        *names.entry(doc.filename.clone()).or_insert(0usize) += 1;
    }
    for doc in documents {
        let filename = doc.filename.clone();
        let result = async {
            document_processor::validate_filename(&filename)?;
            if names[&filename] != 1 {
                bail!("Nombre de documento duplicado");
            }
            let document = document_processor::prepare(config, doc)?;
            ingest_document(config, document).await
        }
        .await;
        results.push(DocumentIngestResult { filename, result });
    }
    results
}

async fn ingest_document(config: &Config, document: Document) -> Result<IngestReport> {
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
    let exists = client.collection_exists(collection.as_str()).await?;
    let binding = if exists {
        qdrant::collection_binding(&client, config, &collection).await?
    } else {
        qdrant::CollectionBinding::new(config, identity.clone())
    };
    binding.validate_model(config, &identity)?;
    binding.validate_source(config)?;
    let corpus = binding.corpus_id()?;
    let manifest = if exists {
        index_state::read_manifest(&client, config, &collection).await?
    } else {
        index_state::Manifest::new()
    };
    let pipeline = index_state::pipeline_version(config, &identity);
    let plan = plan::build(
        config,
        std::slice::from_ref(&document),
        &manifest,
        &corpus,
        &pipeline,
    );
    if !plan.changed.is_empty() {
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
    if !exists {
        qdrant::create_collection(&client, config, &collection, &binding).await?;
    }
    let chunks_written =
        write::documents(&client, &model, &collection, &pipeline, &plan.changed).await?;
    let expected = plan.expected_chunks;
    // Confirmar los IDs y metadatos nuevos mientras los sobrantes siguen presentes.
    if !plan.changed.is_empty() {
        let confirmed = index_state::read_manifest(&client, config, &collection).await?;
        for document in &plan.changed {
            if !confirmed
                .get(&document.source.filename)
                .is_some_and(|points| {
                    index_state::written(
                        document.source,
                        &document.id,
                        &pipeline,
                        document.chunks.len(),
                        points,
                    )
                })
            {
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
        qdrant::delete_points(&client, &collection, &document.obsolete_ids).await?;
    }
    qdrant::verify_count(&client, &collection, expected)
        .await
        .context("El índice no tiene el conteo esperado")?;
    if active.is_none() {
        write::ensure_alias(&client, config, None).await?;
        qdrant::publish_alias(&client, &config.qdrant_alias, &collection).await?;
    }
    write::ensure_alias(&client, config, Some(&collection)).await?;
    Ok(IngestReport {
        chunks: expected,
        active_collection: collection,
        files_new: plan.files_new,
        files_updated: plan.files_updated,
        files_unchanged: plan.files_unchanged,
        chunks_written,
    })
}
