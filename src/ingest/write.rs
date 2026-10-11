use super::{index_state, plan};
use crate::{Config, qdrant, tei::TeiModel};
use anyhow::{Context, Result, bail};
use qdrant_client::{Payload, qdrant::PointStruct};
use rig::embeddings::EmbeddingsBuilder;

pub(super) async fn documents(
    client: &qdrant_client::Qdrant,
    model: &TeiModel,
    collection: &str,
    pipeline: &str,
    documents: &[plan::PendingDocument<'_>],
    progress: Option<&crate::ingestions::Progress>,
) -> Result<usize> {
    let mut chunks_written = 0;
    for document in documents {
        for batch in document.chunks.chunks(TeiModel::MAX_DOCUMENTS) {
            if let Some(progress) = progress {
                progress.check()?;
            }
            let embedded = EmbeddingsBuilder::new(model.embeddings())
                .documents(batch.to_vec())?
                .build()
                .await
                .context("Falló la generación de embeddings")?;
            let mut points = Vec::with_capacity(batch.len());
            for (chunk, embeddings) in embedded {
                if embeddings.len() != 1 {
                    bail!("Cada fragmento debe producir exactamente un embedding");
                }
                let mut payload = serde_json::to_value(&chunk)?;
                let fields = payload
                    .as_object_mut()
                    .context("El fragmento no es un objeto JSON")?;
                if let Some(metadata) = fields.get_mut("metadata") {
                    crate::documents::prepare_qdrant_metadata(metadata);
                }
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
                    index_state::chunk_id(&document.id, chunk.native.chunk_index),
                    vector,
                    Payload::try_from(payload)?,
                ));
            }
            if let Some(progress) = progress {
                progress.check()?;
            }
            qdrant::upsert_points(client, collection, points)
                .await
                .context("Falló la escritura en Qdrant")?;
            chunks_written += batch.len();
        }
    }
    Ok(chunks_written)
}

pub(super) async fn ensure_alias(
    client: &qdrant_client::Qdrant,
    config: &Config,
    expected: Option<&str>,
) -> Result<()> {
    if qdrant::active_collection(client, &config.qdrant_alias)
        .await?
        .as_deref()
        != expected
    {
        bail!("El alias activo cambió durante la ingesta; se cancela la operación");
    }
    Ok(())
}
