use crate::{
    Config, config::EMBEDDING_PREPROCESSING, documents::Document, embeddings::ModelIdentity,
};
use anyhow::{Context, Result, bail};
use qdrant_client::{
    Payload, Qdrant,
    qdrant::{PayloadIncludeSelector, PointId, ScrollPointsBuilder},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    path::{Component, Path},
};
use uuid::Uuid;

pub(super) type Manifest = BTreeMap<String, Vec<StoredPoint>>;

#[derive(Deserialize)]
pub(super) struct StoredChunk {
    pub relative_path: String,
    pub content_hash: String,
    pub chunk_index: usize,
    pub embedding_version: String,
    pub embedding_model: String,
    pub embedding_dimension: usize,
    pub embedding_preprocessing: String,
    #[serde(default)]
    pub document_id: String,
    #[serde(default)]
    pub pipeline_version: String,
    #[serde(default)]
    pub document_chunk_count: usize,
}

pub(super) struct StoredPoint {
    pub id: PointId,
    pub chunk: StoredChunk,
}

pub(super) fn document_id(corpus: &Uuid, relative_path: &str) -> Uuid {
    Uuid::new_v5(corpus, relative_path.as_bytes())
}

pub(super) fn chunk_id(document: &Uuid, chunk_index: usize) -> PointId {
    Uuid::new_v5(document, &(chunk_index as u64).to_be_bytes())
        .to_string()
        .into()
}

pub(super) fn pipeline_version(config: &Config, identity: &ModelIdentity) -> String {
    // Actualizar las versiones al cambiar la extracción o la fragmentación.
    // Una integración de Docling debe incluir aquí su versión y configuración.
    let processing = serde_json::json!({
        "schema": 1,
        "extractor": "utf8-txt-md-v1",
        "chunker": "paragraphs-headings-whitespace-v1",
        "target": config.chunk_target_tokens,
        "maximum": config.chunk_max_tokens,
        "overlap": config.chunk_overlap_tokens,
        "embedding_model": config.embedding_model,
        "embedding_identity": identity,
        "embedding_dimension": config.embedding_dimension,
        "embedding_preprocessing": EMBEDDING_PREPROCESSING,
        "truncate": false,
    });
    format!("{:x}", Sha256::digest(processing.to_string().as_bytes()))
}

pub(super) fn stable_collection(config: &Config) -> String {
    let identity = Uuid::new_v5(&Uuid::NAMESPACE_URL, config.qdrant_alias.as_bytes());
    format!("rag_{}_{}", config.embedding_version(), identity.simple())
}

pub(super) fn writer_lock(config: &Config) -> Result<File> {
    let endpoint = reqwest::Url::parse(&config.qdrant_url).context("QDRANT_URL inválida")?;
    let key = serde_json::json!([endpoint.as_str().trim_end_matches('/'), config.qdrant_alias]);
    let hash = format!("{:x}", Sha256::digest(key.to_string().as_bytes()));
    let path = std::env::temp_dir().join(format!("rag-ingest-{hash}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .context("No se pudo abrir el bloqueo local de ingesta")?;
    file.try_lock().with_context(|| {
        format!(
            "No se pudo bloquear la ingesta; comprueba que no haya otra ejecución para {} ({})",
            config.qdrant_alias,
            path.display()
        )
    })?;
    // Conservar el archivo evita crear otro inode mientras un escritor lo bloquea.
    Ok(file)
}

pub(super) async fn read_manifest(
    client: &Qdrant,
    config: &Config,
    collection: &str,
) -> Result<Manifest> {
    let fields = [
        "relative_path",
        "content_hash",
        "chunk_index",
        "embedding_version",
        "embedding_model",
        "embedding_dimension",
        "embedding_preprocessing",
        "document_id",
        "pipeline_version",
        "document_chunk_count",
    ];
    let mut manifest: Manifest = BTreeMap::new();
    let mut offset = None;
    loop {
        let mut request = ScrollPointsBuilder::new(collection)
            .limit(256)
            .with_payload(PayloadIncludeSelector {
                fields: fields.iter().map(|field| field.to_string()).collect(),
            })
            .with_vectors(false);
        if let Some(offset) = offset {
            request = request.offset(offset);
        }
        let response = client
            .scroll(request)
            .await
            .context("No se pudo leer el estado de los documentos en Qdrant")?;
        for point in response.result {
            let id = point.id.context("Qdrant devolvió un punto sin ID")?;
            let chunk: StoredChunk = serde_json::from_value(Payload::from(point.payload).into())
                .context(
                    "La colección contiene puntos ajenos al formato de este RAG; no se modifica",
                )?;
            if chunk.embedding_version != config.embedding_version()
                || chunk.embedding_model != config.embedding_model
                || chunk.embedding_dimension != config.embedding_dimension
                || chunk.embedding_preprocessing != EMBEDDING_PREPROCESSING
            {
                bail!("La colección contiene embeddings incompatibles; no se modifica");
            }
            if chunk.relative_path.is_empty()
                || !Path::new(&chunk.relative_path)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                bail!("La colección contiene una ruta relativa inválida; no se modifica");
            }
            manifest
                .entry(chunk.relative_path.clone())
                .or_default()
                .push(StoredPoint { id, chunk });
        }
        offset = response.next_page_offset;
        if offset.is_none() {
            break;
        }
    }
    Ok(manifest)
}

pub(super) fn unchanged(
    document: &Document,
    id: &Uuid,
    pipeline: &str,
    points: &[StoredPoint],
) -> bool {
    written(document, id, pipeline, points.len(), points)
}

pub(super) fn written(
    document: &Document,
    id: &Uuid,
    pipeline: &str,
    expected: usize,
    points: &[StoredPoint],
) -> bool {
    if expected == 0 {
        return false;
    }
    let document_id = id.to_string();
    let mut positions = BTreeSet::new();
    for point in points {
        let chunk = &point.chunk;
        // Durante una actualización todavía pueden existir IDs antiguos o sobrantes.
        if chunk.chunk_index >= expected || point.id != chunk_id(id, chunk.chunk_index) {
            continue;
        }
        if chunk.content_hash != document.content_hash
            || chunk.document_id != document_id
            || chunk.pipeline_version != pipeline
            || chunk.document_chunk_count != expected
        {
            return false;
        }
        positions.insert(chunk.chunk_index);
    }
    positions.len() == expected
}
