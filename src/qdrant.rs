use crate::{
    Config,
    tei::{ModelIdentity, TeiModel},
};
use anyhow::{Context, Result, bail};
use qdrant_client::{
    Qdrant,
    qdrant::{
        CountPointsBuilder, CreateAliasBuilder, CreateCollectionBuilder, DeletePointsBuilder,
        Distance, PointId, PointStruct, PointsIdsList, PointsOperationResponse, QueryPointsBuilder,
        UpdateStatus, UpsertPointsBuilder, VectorParamsBuilder,
        vectors_config::Config as VectorConfig,
    },
};
use rig_qdrant::QdrantVectorStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

const METADATA_KEY: &str = "rag_ingest";
const SCHEMA_VERSION: u32 = 3;
const CORPUS_SOURCE: &str = "docling-manual";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CollectionBinding {
    schema_version: u32,
    corpus_id: String,
    source: String,
    alias: String,
    embedding_version: String,
    model_identity: ModelIdentity,
}

impl CollectionBinding {
    pub(crate) fn new(config: &Config, model_identity: ModelIdentity) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            corpus_id: Uuid::new_v4().to_string(),
            source: CORPUS_SOURCE.into(),
            alias: config.qdrant_alias.clone(),
            embedding_version: config.embedding_version(),
            model_identity,
        }
    }

    pub(crate) fn corpus_id(&self) -> Result<Uuid> {
        Uuid::parse_str(&self.corpus_id).context("Identidad del corpus inválida")
    }

    pub(crate) fn validate_model(&self, config: &Config, identity: &ModelIdentity) -> Result<()> {
        if !matches!(self.schema_version, 2 | SCHEMA_VERSION) {
            bail!("Versión de metadatos del índice no compatible");
        }
        self.corpus_id()?;
        if self.embedding_version != config.embedding_version() || &self.model_identity != identity
        {
            bail!(
                "El modelo, revisión o configuración de embeddings no coincide con el índice activo"
            );
        }
        Ok(())
    }

    pub(crate) fn validate_source(&self, config: &Config) -> Result<()> {
        if self.source != CORPUS_SOURCE || self.alias != config.qdrant_alias {
            bail!("La colección no corresponde al corpus Docling y al alias configurado");
        }
        Ok(())
    }

    pub(crate) fn validate_ingestion(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!("El esquema del índice no es compatible con la ingesta actual");
        }
        Ok(())
    }

    fn metadata(&self) -> Result<HashMap<String, serde_json::Value>> {
        Ok(HashMap::from([(
            METADATA_KEY.to_string(),
            serde_json::to_value(self)?,
        )]))
    }
}

pub(crate) fn client(config: &Config) -> Result<Qdrant> {
    Qdrant::from_url(&config.qdrant_url)
        .build()
        .context("No se pudo crear el cliente Qdrant")
}

pub(crate) fn store(
    client: Qdrant,
    model: TeiModel,
    collection: &str,
) -> QdrantVectorStore<TeiModel> {
    QdrantVectorStore::new(
        client,
        model,
        QueryPointsBuilder::new(collection)
            .with_payload(true)
            .build(),
    )
}

pub(crate) async fn active_collection(client: &Qdrant, alias: &str) -> Result<Option<String>> {
    crate::logging::operation("qdrant", "active_collection", async {
        let aliases = client
            .list_aliases()
            .await
            .context("No se pudieron consultar los alias de Qdrant")?;
        Ok(aliases
            .aliases
            .into_iter()
            .find(|a| a.alias_name == alias)
            .map(|a| a.collection_name))
    })
    .await
}

pub(crate) async fn create_collection(
    client: &Qdrant,
    config: &Config,
    collection: &str,
    binding: &CollectionBinding,
) -> Result<()> {
    crate::logging::operation("qdrant", "create_collection", async {
        let response = client
            .create_collection(
                CreateCollectionBuilder::new(collection)
                    .vectors_config(VectorParamsBuilder::new(
                        config.embedding_dimension as u64,
                        Distance::Cosine,
                    ))
                    .metadata(binding.metadata()?),
            )
            .await
            .context("No se pudo crear la colección nueva")?;
        if !response.result {
            bail!("Qdrant no confirmó la creación de la colección");
        }
        Ok(())
    })
    .await
}

pub(crate) async fn collection_binding(
    client: &Qdrant,
    config: &Config,
    collection: &str,
) -> Result<CollectionBinding> {
    crate::logging::operation("qdrant", "collection_binding", async {
    if !collection.starts_with(&format!("rag_{}_", config.embedding_version())) {
        bail!("La colección no corresponde a la configuración de embeddings");
    }
    let info = client
        .collection_info(collection)
        .await?
        .result
        .context("Qdrant no devolvió la información de la colección")?;
    let collection_config = info
        .config
        .context("Qdrant no devolvió la configuración de la colección")?;
    let vectors = collection_config
        .params
        .and_then(|params| params.vectors_config)
        .and_then(|vectors| vectors.config)
        .context("La colección no tiene configuración de vectores")?;
    match vectors {
        VectorConfig::Params(params)
            if params.size == config.embedding_dimension as u64
                && params.distance == Distance::Cosine as i32 => {}
        _ => bail!(
            "La colección debe usar un vector denso sin nombre, dimensión compatible y distancia coseno"
        ),
    }
    let metadata = collection_config
        .metadata
        .get(METADATA_KEY)
        .context("La colección no tiene metadatos de este RAG")?;
    serde_json::from_value(metadata.clone().into()).context("Metadatos de ingesta inválidos")
    }).await
}

fn confirm_update(response: PointsOperationResponse) -> Result<()> {
    let result = response
        .result
        .context("Qdrant no devolvió el resultado de la escritura")?;
    if result.status != UpdateStatus::Completed as i32 {
        bail!("Qdrant no confirmó la aplicación de la escritura");
    }
    Ok(())
}

pub(crate) async fn upsert_points(
    client: &Qdrant,
    collection: &str,
    points: Vec<PointStruct>,
) -> Result<()> {
    crate::logging::operation("qdrant", "upsert_points", async {
        confirm_update(
            client
                .upsert_points(UpsertPointsBuilder::new(collection, points).wait(true))
                .await?,
        )
    })
    .await
}

pub(crate) async fn delete_points(
    client: &Qdrant,
    collection: &str,
    ids: &[PointId],
) -> Result<()> {
    crate::logging::operation("qdrant", "delete_points", async {
        for batch in ids.chunks(256) {
            confirm_update(
                client
                    .delete_points(
                        DeletePointsBuilder::new(collection)
                            .points(PointsIdsList {
                                ids: batch.to_vec(),
                            })
                            .wait(true),
                    )
                    .await?,
            )?;
        }
        Ok(())
    })
    .await
}

pub(crate) async fn verify_count(client: &Qdrant, collection: &str, expected: usize) -> Result<()> {
    crate::logging::operation("qdrant", "verify_count", async {
        let count = client
            .count(CountPointsBuilder::new(collection).exact(true))
            .await?
            .result
            .context("Qdrant no devolvió el conteo")?
            .count;
        if count != expected as u64 {
            bail!("Qdrant confirmó {count} puntos, pero se esperaban {expected}");
        }
        Ok(())
    })
    .await
}

pub(crate) async fn publish_alias(client: &Qdrant, alias: &str, collection: &str) -> Result<()> {
    crate::logging::operation("qdrant", "publish_alias", async {
        let response = client
            .create_alias(CreateAliasBuilder::new(collection, alias))
            .await
            .context("No se pudo activar el alias de Qdrant")?;
        if !response.result {
            bail!("Qdrant no confirmó la activación del alias");
        }
        Ok(())
    })
    .await
}
