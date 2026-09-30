mod plan;
mod state;

use crate::{Config, documents::read_documents, embeddings::TeiModel, index};
use anyhow::{Context, Result, bail};
use qdrant_client::{Payload, qdrant::PointStruct};
use rig::embeddings::{EmbeddingModel, EmbeddingsBuilder};
use serde::Serialize;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct IngestOptions {
    pub source_dir: PathBuf,
    pub prune: bool,
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
    pub files_new: usize,
    pub files_updated: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    pub chunks_written: usize,
    pub preserved: Vec<String>,
    pub pending_removal: Vec<String>,
}

pub async fn ingest(config: &Config, options: IngestOptions) -> Result<IngestReport> {
    let _writer = state::writer_lock(config)?;
    let root = options
        .source_dir
        .canonicalize()
        .context("No se pudo resolver la carpeta fuente")?;
    let source_root = root
        .to_str()
        .context("La carpeta fuente no es UTF-8")?
        .to_string();
    let inventory = read_documents(&root, config.max_file_bytes)?;
    if inventory.documents.is_empty() {
        bail!("No hay documentos admitidos; se conserva el índice y no se realizan eliminaciones");
    }
    let model = TeiModel::new(config)?;
    // Info identifica el modelo; una ejecución sin cambios no necesita Embed.
    let identity = model
        .check_model()
        .await
        .context("TEI no está listo para la ingesta")?;
    let client = index::client(config)?;
    let active = index::active_collection(&client, &config.qdrant_alias).await?;
    let collection = active
        .clone()
        .unwrap_or_else(|| state::stable_collection(config));
    let exists = client.collection_exists(collection.as_str()).await?;
    let stored_binding = if exists {
        index::collection_binding(&client, config, &collection).await?
    } else {
        None
    };
    if exists && stored_binding.is_none() && active.is_none() {
        bail!(
            "La colección {} ya existe sin metadatos de este módulo; no se adopta automáticamente",
            collection
        );
    }
    let binding = stored_binding.clone().unwrap_or_else(|| {
        index::CollectionBinding::new(config, source_root.clone(), identity.clone())
    });
    binding.validate_model(config, &identity)?;
    binding.validate_source(config, &source_root)?;
    let corpus = Uuid::parse_str(&binding.corpus_id)?;
    let manifest = if exists {
        if stored_binding.is_none()
            && client.list_aliases().await?.aliases.iter().any(|alias| {
                alias.collection_name == collection && alias.alias_name != config.qdrant_alias
            })
        {
            bail!("La colección anterior tiene otros alias; no se migra automáticamente");
        }
        state::read_manifest(&client, config, &collection).await?
    } else {
        state::Manifest::new()
    };
    let pipeline = state::pipeline_version(config, &identity);
    let plan = plan::build(
        config,
        &inventory.documents,
        &inventory,
        &manifest,
        &corpus,
        &pipeline,
    )?;
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
    ensure_alias(&client, config, active.as_deref()).await?;
    if !exists {
        index::create_collection(&client, config, &collection, &binding).await?;
    } else if stored_binding.is_none() {
        // Los puntos antiguos carecen de la versión del pipeline y se reprocesan.
        index::bind_collection(&client, &collection, &binding).await?;
    }
    let chunks_written =
        write_documents(&client, &model, &collection, &pipeline, &plan.changed).await?;
    let mut expected = plan.expected_chunks;
    // Confirmar los IDs y metadatos nuevos mientras los sobrantes siguen presentes.
    if !plan.changed.is_empty() {
        let confirmed = state::read_manifest(&client, config, &collection).await?;
        for document in &plan.changed {
            if !confirmed
                .get(&document.source.relative_path)
                .is_some_and(|points| {
                    state::written(
                        document.source,
                        &document.id,
                        &pipeline,
                        document.chunks.len(),
                        points,
                    )
                })
            {
                bail!(
                    "Qdrant no confirmó todos los fragmentos de {}; se conservan los puntos sobrantes y puede reintentarse ingest",
                    document.source.relative_path
                );
            }
        }
    }
    // Todas las escrituras terminaron antes de retirar cualquier punto.
    ensure_alias(&client, config, active.as_deref()).await?;
    for document in &plan.changed {
        if document.obsolete_ids.is_empty() {
            continue;
        }
        eprintln!(
            "Qdrant: retirar {} fragmentos obsoletos de {:?} en la colección {:?}",
            document.obsolete_ids.len(),
            document.source.relative_path,
            collection
        );
        index::delete_points(&client, &collection, &document.obsolete_ids).await?;
    }
    if options.prune {
        if !root.is_dir() || root.canonicalize()? != root {
            bail!(
                "La carpeta fuente desapareció o cambió durante la ingesta; se cancela la eliminación de documentos"
            );
        }
        for path in &plan.missing {
            match std::fs::symlink_metadata(root.join(path)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => bail!(
                    "La ruta {} reapareció durante la ingesta; se cancela la eliminación de documentos",
                    path
                ),
                Err(error) => {
                    return Err(error).context(
                        "No se pudo confirmar una ruta ausente; se cancela la eliminación",
                    );
                }
            }
        }
        for path in &plan.missing {
            let points = &manifest[path];
            eprintln!(
                "Qdrant: eliminar documento {:?} de la colección {:?} ({} fragmentos)",
                path,
                collection,
                points.len()
            );
            index::delete_points(
                &client,
                &collection,
                &points
                    .iter()
                    .map(|point| point.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
            expected -= points.len();
        }
    }
    index::verify_count(&client, &collection, expected)
        .await
        .context("El índice no tiene el conteo esperado; vuelve a ejecutar ingest")?;
    if active.is_none() {
        ensure_alias(&client, config, None).await?;
        index::publish_alias(&client, &config.qdrant_alias, &collection).await?;
    }
    ensure_alias(&client, config, Some(&collection)).await?;
    let mut skipped = inventory.skipped;
    skipped.extend(plan.skipped);
    Ok(IngestReport {
        files_seen: inventory.files_seen,
        files_indexed: plan.files_new + plan.files_updated + plan.files_unchanged,
        files_skipped: skipped.len(),
        skipped,
        chunks: expected,
        active_collection: collection,
        files_new: plan.files_new,
        files_updated: plan.files_updated,
        files_unchanged: plan.files_unchanged,
        files_removed: if options.prune { plan.missing.len() } else { 0 },
        chunks_written,
        preserved: plan.preserved,
        pending_removal: if options.prune {
            Vec::new()
        } else {
            plan.missing
        },
    })
}

async fn write_documents(
    client: &qdrant_client::Qdrant,
    model: &TeiModel,
    collection: &str,
    pipeline: &str,
    documents: &[plan::PendingDocument<'_>],
) -> Result<usize> {
    let mut chunks_written = 0;
    for document in documents {
        for batch in document.chunks.chunks(TeiModel::MAX_DOCUMENTS) {
            let embedded = EmbeddingsBuilder::new(model.clone())
                .documents(batch.to_vec())?
                .build()
                .await
                .context("Falló la generación de embeddings; vuelve a ejecutar ingest para completar la sincronización")?;
            let mut points = Vec::with_capacity(batch.len());
            for (chunk, embeddings) in embedded {
                if embeddings.len() != 1 {
                    bail!("Cada fragmento debe producir exactamente un embedding");
                }
                let mut payload = serde_json::to_value(&chunk)?;
                let fields = payload
                    .as_object_mut()
                    .context("El fragmento no es un objeto JSON")?;
                fields.insert("document_id".into(), document.id.to_string().into());
                fields.insert("pipeline_version".into(), pipeline.into());
                fields.insert("document_chunk_count".into(), document.chunks.len().into());
                let vector: Vec<f32> = embeddings
                    .into_iter()
                    .next()
                    .context("Falta embedding")?
                    .vec
                    .into_iter()
                    .map(|value| value as f32)
                    .collect();
                points.push(PointStruct::new(
                    state::chunk_id(&document.id, chunk.chunk_index),
                    vector,
                    Payload::try_from(payload)?,
                ));
            }
            index::upsert_points(client, collection, points)
                .await
                .context("Falló la escritura en Qdrant; vuelve a ejecutar ingest para completar la sincronización")?;
            chunks_written += batch.len();
        }
    }
    Ok(chunks_written)
}

async fn ensure_alias(
    client: &qdrant_client::Qdrant,
    config: &Config,
    expected: Option<&str>,
) -> Result<()> {
    if index::active_collection(client, &config.qdrant_alias)
        .await?
        .as_deref()
        != expected
    {
        bail!("El alias activo cambió durante la ingesta; se cancela la operación");
    }
    Ok(())
}
