use crate::Config;
use anyhow::{Result, bail};
use rig::embeddings::{Embedding, EmbeddingError, EmbeddingModel};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct TeiModel {
    http: reqwest::Client,
    url: String,
    model: String,
    dimension: usize,
}

#[derive(Serialize)]
struct TeiRequest<'a> {
    inputs: &'a [String],
    truncate: bool,
}

#[derive(Deserialize)]
struct TeiInfo {
    model_id: Option<String>,
}

impl TeiModel {
    pub fn new(config: &Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: config.tei_url.trim_end_matches('/').to_string(),
            model: config.embedding_model.clone(),
            dimension: config.embedding_dimension,
        }
    }

    pub async fn preflight(&self) -> Result<()> {
        let response = self
            .http
            .get(format!("{}/info", self.url))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("TEI no responde en /info: {}", response.status());
        }
        let info: TeiInfo = response.json().await?;
        if info.model_id.as_deref() != Some(self.model.as_str()) {
            bail!("El modelo cargado por TEI no coincide con EMBEDDING_MODEL");
        }
        self.embed_text("comprobación de dimensiones").await?;
        Ok(())
    }
}

impl EmbeddingModel for TeiModel {
    type Client = Self;
    const MAX_DOCUMENTS: usize = 16;

    fn make(client: &Self::Client, model: impl Into<String>, dims: Option<usize>) -> Self {
        let mut copy = client.clone();
        copy.model = model.into();
        if let Some(dims) = dims {
            copy.dimension = dims;
        }
        copy
    }

    fn ndims(&self) -> usize {
        self.dimension
    }

    async fn embed_texts(
        &self,
        texts: impl IntoIterator<Item = String> + Send,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        let texts: Vec<String> = texts.into_iter().collect();
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .http
            .post(format!("{}/embed", self.url))
            .timeout(Duration::from_secs(120))
            .json(&TeiRequest {
                inputs: &texts,
                truncate: false,
            })
            .send()
            .await
            .map_err(|e| EmbeddingError::ResponseError(format!("TEI: {e}")))?;
        if !response.status().is_success() {
            return Err(EmbeddingError::ResponseError(format!(
                "TEI devolvió {}; comprueba el límite de tokens y el estado del servidor",
                response.status()
            )));
        }
        let vectors: Vec<Vec<f64>> = response.json().await.map_err(|e| {
            EmbeddingError::ResponseError(format!("Respuesta de TEI inválida: {e}"))
        })?;
        if vectors.len() != texts.len() {
            return Err(EmbeddingError::ResponseError(
                "TEI devolvió un número incorrecto de vectores".into(),
            ));
        }
        vectors
            .into_iter()
            .zip(texts)
            .map(|(vec, document)| {
                if vec.len() != self.dimension || vec.iter().any(|v| !v.is_finite()) {
                    return Err(EmbeddingError::ResponseError(format!(
                        "TEI devolvió un vector inválido; se esperaban {} dimensiones finitas",
                        self.dimension
                    )));
                }
                Ok(Embedding { document, vec })
            })
            .collect()
    }
}
