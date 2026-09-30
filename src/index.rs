use crate::{
    Config,
    embeddings::{ModelIdentity, TeiModel},
};
use anyhow::{Context, Result, bail};
use qdrant_client::{
    Qdrant,
    qdrant::{
        CountPointsBuilder, CreateAliasBuilder, CreateCollectionBuilder, DeletePointsBuilder,
        Distance, PointId, PointStruct, PointsIdsList, PointsOperationResponse, QueryPointsBuilder,
        UpdateCollectionBuilder, UpdateStatus, UpsertPointsBuilder, VectorParamsBuilder,
        vectors_config::Config as VectorConfig,
    },
};
use rig_qdrant::QdrantVectorStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

const METADATA_KEY: &str = "rag_ingest";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CollectionBinding {
    schema_version: u32,
    pub corpus_id: String,
    pub source_root: String,
    pub alias: String,
    embedding_version: String,
    model_identity: ModelIdentity,
}

impl CollectionBinding {
    pub fn new(config: &Config, source_root: String, model_identity: ModelIdentity) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            corpus_id: Uuid::new_v4().to_string(),
            source_root,
            alias: config.qdrant_alias.clone(),
            embedding_version: config.embedding_version(),
            model_identity,
        }
    }

    pub fn validate_model(&self, config: &Config, identity: &ModelIdentity) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!("Versión de metadatos del índice no compatible");
        }
        Uuid::parse_str(&self.corpus_id).context("Identidad del corpus inválida")?;
        if self.embedding_version != config.embedding_version() || &self.model_identity != identity
        {
            bail!(
                "El modelo, revisión o configuración de embeddings cambió; se requiere una reconstrucción explícita del índice"
            );
        }
        Ok(())
    }

    pub fn validate_source(&self, config: &Config, source_root: &str) -> Result<()> {
        if self.source_root != source_root || self.alias != config.qdrant_alias {
            bail!(
                "La colección pertenece al alias {} y a la carpeta {}; usa su carpeta original o un alias distinto para otro corpus",
                self.alias,
                self.source_root
            );
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
    let aliases = client
        .list_aliases()
        .await
        .context("No se pudieron consultar los alias de Qdrant")?;
    Ok(aliases
        .aliases
        .into_iter()
        .find(|a| a.alias_name == alias)
        .map(|a| a.collection_name))
}

pub(crate) async fn create_collection(
    client: &Qdrant,
    config: &Config,
    collection: &str,
    binding: &CollectionBinding,
) -> Result<()> {
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
}

pub(crate) async fn collection_binding(
    client: &Qdrant,
    config: &Config,
    collection: &str,
) -> Result<Option<CollectionBinding>> {
    if !collection.starts_with(&format!("rag_{}_", config.embedding_version())) {
        bail!(
            "La colección no corresponde a la configuración de embeddings; se requiere una reconstrucción explícita"
        );
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
    collection_config
        .metadata
        .get(METADATA_KEY)
        .map(|value| {
            serde_json::from_value(value.clone().into()).context("Metadatos de ingesta inválidos")
        })
        .transpose()
}

pub(crate) async fn bind_collection(
    client: &Qdrant,
    collection: &str,
    binding: &CollectionBinding,
) -> Result<()> {
    let response = client
        .update_collection(UpdateCollectionBuilder::new(collection).metadata(binding.metadata()?))
        .await
        .context("No se pudo vincular la colección a la carpeta fuente")?;
    if !response.result {
        bail!("Qdrant no confirmó los metadatos de la colección");
    }
    Ok(())
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
    confirm_update(
        client
            .upsert_points(UpsertPointsBuilder::new(collection, points).wait(true))
            .await?,
    )
}

pub(crate) async fn delete_points(
    client: &Qdrant,
    collection: &str,
    ids: &[PointId],
) -> Result<()> {
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
}

pub(crate) async fn verify_count(client: &Qdrant, collection: &str, expected: usize) -> Result<()> {
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
}

pub(crate) async fn publish_alias(client: &Qdrant, alias: &str, collection: &str) -> Result<()> {
    let response = client
        .create_alias(CreateAliasBuilder::new(collection, alias))
        .await
        .context("No se pudo activar el alias de Qdrant")?;
    if !response.result {
        bail!("Qdrant no confirmó la activación del alias");
    }
    Ok(())
}
