use super::index_state::{self, StoredPoint};
use crate::{
    Config,
    config::EMBEDDING_PREPROCESSING,
    docling::FileReport,
    qdrant,
    tei::{ModelIdentity, TeiModel},
};
use anyhow::{Context, Result};
use qdrant_client::{
    Payload,
    qdrant::{Condition, Filter, PayloadIncludeSelector, ScrollPointsBuilder},
};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

/// Read document metadata and detect conflicting identities at its current locator.
pub(crate) async fn original_unchanged(
    config: &Config,
    report: &FileReport,
    progress: &crate::ingestions::Progress,
) -> Result<Option<usize>> {
    let Some(_) = report.original_sha256 else {
        return Ok(None);
    };
    let _writer = index_state::writer_lock(config)?;
    let identity = TeiModel::new(config)?.check_model().await?;
    let client = qdrant::client(config)?;
    let active = qdrant::active_collection(&client, &config.qdrant_alias).await?;
    let collection = active
        .clone()
        .unwrap_or_else(|| index_state::stable_collection(config));
    if active.is_none() && !client.collection_exists(collection.as_str()).await? {
        return Ok(None);
    }
    let binding = qdrant::collection_binding(&client, config, &collection).await?;
    binding.validate_source(config)?;
    binding.validate_ingestion()?;
    binding.validate_model(config, &identity)?;
    let id = report.document_id;
    progress.check()?;
    let fields = [
        "filename",
        "source_key",
        "content_hash",
        "chunk_index",
        "embedding_version",
        "embedding_model",
        "embedding_dimension",
        "embedding_preprocessing",
        "document_id",
        "pipeline_version",
        "document_chunk_count",
        "metadata.rag.original_sha256",
        "metadata.rag.profile",
    ];
    let mut offset = None;
    let mut points = Vec::new();
    loop {
        let mut request = ScrollPointsBuilder::new(&collection)
            .limit(256)
            .filter(Filter::should([
                Condition::matches("document_id", id.to_string()),
                Condition::matches("source_key", report.source_key.clone()),
            ]))
            .with_payload(PayloadIncludeSelector {
                fields: fields.iter().map(|s| s.to_string()).collect(),
            })
            .with_vectors(false);
        if let Some(value) = offset {
            request = request.offset(value);
        }
        let response = client
            .scroll(request)
            .await
            .context("No se pudo comprobar el original en Qdrant")?;
        for point in response.result {
            // Incomplete metadata is insufficient evidence of an unchanged document.
            let Some(id) = point.id else {
                return Ok(None);
            };
            let Ok(chunk) = serde_json::from_value(Payload::from(point.payload).into()) else {
                return Ok(None);
            };
            points.push(StoredPoint { id, chunk });
        }
        offset = response.next_page_offset;
        if offset.is_none() {
            break;
        }
    }
    index_state::ensure_source_identity(id, &report.source_key, &points)?;
    let result = complete_original(config, report, &identity, &id, &points);
    super::write::ensure_alias(&client, config, active.as_deref()).await?;
    if result.is_some() && active.is_none() {
        // Recover a first document written before the alias was published.
        qdrant::verify_count(&client, &collection, points.len()).await?;
        progress.check()?;
        qdrant::publish_alias(&client, &config.qdrant_alias, &collection).await?;
    }
    Ok(result)
}

fn complete_original(
    config: &Config,
    report: &FileReport,
    identity: &ModelIdentity,
    id: &Uuid,
    points: &[StoredPoint],
) -> Option<usize> {
    let first = &points.first()?.chunk;
    let expected = first.document_chunk_count;
    if expected == 0 || expected != points.len() || first.content_hash.len() != 64 {
        return None;
    }
    if !first.content_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let pipeline = index_state::pipeline_version(config, identity);
    let model = serde_json::to_value(identity).ok()?;
    let version = config.embedding_version();
    let mut positions = BTreeSet::new();
    let profile = first.metadata.pointer("/rag/profile")?;
    for point in points {
        let c = &point.chunk;
        if c.filename != report.filename
            || c.source_key != report.source_key
            || c.document_id != *id
            || c.chunk_index >= expected
            || point.id != index_state::chunk_id(id, c.chunk_index)
            || !positions.insert(c.chunk_index)
            || c.document_chunk_count != expected
            || c.content_hash != first.content_hash
            || c.pipeline_version != pipeline
            || c.embedding_version != version
            || c.embedding_model != config.embedding_model
            || c.embedding_dimension != config.embedding_dimension
            || c.embedding_preprocessing != EMBEDDING_PREPROCESSING
            || c.metadata
                .pointer("/rag/original_sha256")
                .and_then(Value::as_str)
                != report.original_sha256.as_deref()
            || c.metadata.pointer("/rag/profile/conversion") != Some(&report.profile)
            || c.metadata.pointer("/rag/profile/embedding_identity") != Some(&model)
            || c.metadata.pointer("/rag/profile") != Some(profile)
        {
            return None;
        }
    }
    Some(expected)
}
