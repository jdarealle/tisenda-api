use crate::Config;
use anyhow::{Context, Result, bail};
use futures_util::future::try_join_all;
use rig::embeddings::{Embedding, EmbeddingError, EmbeddingModel};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use tonic::{
    Request, Response, Status,
    transport::{Channel, Endpoint},
};

mod proto {
    tonic::include_proto!("tei.v1");
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const INFO_DEADLINE: Duration = Duration::from_secs(15);
const EMBED_DEADLINE: Duration = Duration::from_secs(120);
// Cubre también la espera del cliente y la recepción del cuerpo. Un vencimiento
// aquí se informa como protección local, separado del estado devuelto por gRPC.
const LOCAL_TIMEOUT_GRACE: Duration = Duration::from_secs(10);

async fn tei_rpc<M, T, F>(
    rpc: &'static str,
    deadline: Duration,
    message: M,
    call: impl FnOnce(Request<M>) -> F,
) -> Result<T>
where
    F: Future<Output = Result<Response<T>, Status>>,
{
    crate::logging::operation("tei", rpc, async {
        let mut request = Request::new(message);
        request.set_timeout(deadline);
        let local_timeout = deadline + LOCAL_TIMEOUT_GRACE;
        let response = tokio::time::timeout(local_timeout, call(request))
            .await
            .with_context(|| {
                format!(
                    "TEI {rpc}: venció la protección local de {} s (deadline gRPC de {} s)",
                    local_timeout.as_secs(),
                    deadline.as_secs()
                )
            })?
            .with_context(|| format!("TEI gRPC {rpc} devolvió un error"))?;
        Ok(response.into_inner())
    })
    .await
}

#[derive(Clone)]
pub(crate) struct TeiModel {
    channel: Channel,
    model: String,
    dimension: usize,
    revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ModelIdentity {
    revision: String,
    dtype: String,
}

impl TeiModel {
    pub(crate) fn new(config: &Config) -> Result<Self> {
        let channel = Endpoint::from_shared(config.tei_url.trim_end_matches('/').to_string())
            .context("TEI_URL debe ser una URL gRPC válida")?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect_lazy();
        Ok(Self {
            channel,
            model: config.embedding_model.clone(),
            dimension: config.embedding_dimension,
            revision: config.embedding_revision.clone(),
        })
    }

    async fn info(&self) -> Result<proto::InfoResponse> {
        let mut client = proto::info_client::InfoClient::new(self.channel.clone());
        tei_rpc(
            "Info",
            INFO_DEADLINE,
            proto::InfoRequest {},
            move |request| async move { client.info(request).await },
        )
        .await
    }

    pub(crate) async fn input_limit(&self) -> Result<usize> {
        let info = self.info().await?;
        if info.max_input_length == 0 {
            bail!("TEI no informa un max_input_length válido");
        }
        Ok(info.max_input_length as usize)
    }

    /// Same input and implicit prompt as Embed; special tokens count toward the limit.
    pub(crate) async fn token_count(&self, text: &str) -> Result<usize> {
        let mut client = proto::tokenize_client::TokenizeClient::new(self.channel.clone())
            .max_decoding_message_size(64 * 1024 * 1024);
        let response = tei_rpc(
            "Tokenize",
            EMBED_DEADLINE,
            proto::EncodeRequest {
                inputs: text.to_owned(),
                add_special_tokens: true,
                prompt_name: None,
            },
            move |request| async move { client.tokenize(request).await },
        )
        .await?;
        Ok(response.tokens.len())
    }

    /// None means the request exceeds TEI's character guard even before tokenization.
    pub(crate) async fn chunk_counts(
        &self,
        chunks: &[crate::DoclingChunk],
        limit: usize,
    ) -> Result<Vec<Option<usize>>> {
        let mut counts = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            // TEI 1.9.4 Tokenize rejects inputs above max_input_length * 250 characters.
            if chunk.text.chars().count() > limit.saturating_mul(250) {
                counts.push(None);
            } else {
                counts.push(Some(self.token_count(&chunk.text).await?));
            }
        }
        Ok(counts)
    }

    pub(crate) async fn check_model(&self) -> Result<ModelIdentity> {
        let info = self.info().await?;
        if info.model_id != self.model {
            bail!("El modelo cargado por TEI no coincide con EMBEDDING_MODEL");
        }
        let reported_revision = info.model_sha.filter(|revision| !revision.is_empty());
        if let (Some(expected), Some(actual)) = (&self.revision, &reported_revision)
            && expected != actual
        {
            bail!("La revisión cargada por TEI no coincide con EMBEDDING_REVISION");
        }
        let revision = reported_revision.or_else(|| self.revision.clone()).context(
            "TEI no informa model_sha; fija la revisión del modelo y define EMBEDDING_REVISION para identificarla",
        )?;
        Ok(ModelIdentity {
            revision,
            dtype: info.model_dtype,
        })
    }

    pub(crate) async fn preflight(&self) -> Result<ModelIdentity> {
        let identity = self.check_model().await?;
        self.embed_text("comprobación de dimensiones").await?;
        Ok(identity)
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
        let dimension = self.dimension;
        let requests = texts.into_iter().map(|text| {
            let channel = self.channel.clone();
            async move {
                let mut client = proto::embed_client::EmbedClient::new(channel);
                let response = tei_rpc(
                    "Embed",
                    EMBED_DEADLINE,
                    proto::EmbedRequest {
                        inputs: text.clone(),
                        truncate: false,
                        normalize: Some(true),
                    },
                    move |request| async move { client.embed(request).await },
                )
                .await
                .map_err(|error| EmbeddingError::ResponseError(format!("{error:#}")))?;
                let vec: Vec<f64> = response.embeddings.into_iter().map(f64::from).collect();
                if vec.len() != dimension || vec.iter().any(|value| !value.is_finite()) {
                    return Err(EmbeddingError::ResponseError(format!(
                        "TEI devolvió un vector inválido; se esperaban {dimension} dimensiones finitas"
                    )));
                }
                Ok(Embedding {
                    document: text,
                    vec,
                })
            }
        });
        try_join_all(requests).await
    }
}
