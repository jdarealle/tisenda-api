use crate::{Config, embeddings::TeiModel};
use anyhow::{Context, Result, bail};
use qdrant_client::{
    Qdrant,
    qdrant::{
        CountPointsBuilder, CreateCollectionBuilder, Distance, QueryPointsBuilder,
        VectorParamsBuilder,
    },
};
use rig_qdrant::QdrantVectorStore;

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
) -> Result<()> {
    client
        .create_collection(CreateCollectionBuilder::new(collection).vectors_config(
            VectorParamsBuilder::new(config.embedding_dimension as u64, Distance::Cosine),
        ))
        .await
        .context("No se pudo crear la colección nueva")?;
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

pub(crate) async fn publish_alias(config: &Config, old: Option<&str>, new: &str) -> Result<()> {
    let mut actions = Vec::new();
    if old.is_some() {
        actions.push(serde_json::json!({"delete_alias": {"alias_name": config.qdrant_alias}}));
    }
    actions.push(serde_json::json!({"create_alias": {"collection_name": new, "alias_name": config.qdrant_alias}}));
    let response = reqwest::Client::new()
        .post(format!(
            "{}/collections/aliases",
            config.qdrant_http_url.trim_end_matches('/')
        ))
        .json(&serde_json::json!({"actions": actions}))
        .send()
        .await
        .context("No se pudo cambiar el alias de Qdrant")?;
    if !response.status().is_success() {
        bail!(
            "Qdrant rechazó el cambio de alias: {}",
            response.text().await.unwrap_or_default()
        );
    }
    Ok(())
}
