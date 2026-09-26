use crate::Config;
use anyhow::{Context, Result, bail};
use futures_util::future::try_join_all;
use rig::embeddings::{Embedding, EmbeddingError, EmbeddingModel};
use std::time::Duration;
use tonic::{
    Request,
    client::Grpc,
    codegen::http::uri::PathAndQuery,
    transport::{Channel, Endpoint},
};
use tonic_prost::ProstCodec;

// Estos campos conservan los números del contrato público en proto/tei.proto.
mod proto {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct InfoRequest {}

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct InfoResponse {
        #[prost(string, tag = "4")]
        pub model_id: String,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct EmbedRequest {
        #[prost(string, tag = "1")]
        pub inputs: String,
        #[prost(bool, tag = "2")]
        pub truncate: bool,
        #[prost(bool, optional, tag = "3")]
        pub normalize: Option<bool>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct EmbedResponse {
        #[prost(float, repeated, tag = "1")]
        pub embeddings: Vec<f32>,
    }
}

#[derive(Clone)]
pub(crate) struct TeiModel {
    channel: Channel,
    model: String,
    dimension: usize,
}

impl TeiModel {
    pub fn new(config: &Config) -> Result<Self> {
        let channel = Endpoint::from_shared(config.tei_url.trim_end_matches('/').to_string())
            .context("TEI_URL debe ser una URL gRPC válida")?
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .connect_lazy();
        Ok(Self {
            channel,
            model: config.embedding_model.clone(),
            dimension: config.embedding_dimension,
        })
    }

    async fn info(&self) -> Result<proto::InfoResponse> {
        let mut grpc = Grpc::new(self.channel.clone());
        grpc.ready().await.context("TEI gRPC no está listo")?;
        let response: tonic::Response<proto::InfoResponse> = grpc
            .unary(
                Request::new(proto::InfoRequest {}),
                PathAndQuery::from_static("/tei.v1.Info/Info"),
                ProstCodec::default(),
            )
            .await
            .context("TEI gRPC rechazó Info")?;
        Ok(response.into_inner())
    }

    pub async fn preflight(&self) -> Result<()> {
        let info = tokio::time::timeout(Duration::from_secs(15), self.info())
            .await
            .context("TEI gRPC no responde a Info")??;
        if info.model_id != self.model {
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
        let dimension = self.dimension;
        let requests = texts.into_iter().map(|text| {
            let channel = self.channel.clone();
            async move {
                let mut grpc = Grpc::new(channel);
                let mut request = Request::new(proto::EmbedRequest {
                    inputs: text.clone(),
                    truncate: false,
                    normalize: Some(true),
                });
                request.set_timeout(Duration::from_secs(120));
                let response = tokio::time::timeout(Duration::from_secs(120), async {
                    grpc.ready().await.map_err(|error| {
                        EmbeddingError::ResponseError(format!("TEI gRPC no está listo: {error}"))
                    })?;
                    let response: tonic::Response<proto::EmbedResponse> = grpc
                        .unary(
                            request,
                            PathAndQuery::from_static("/tei.v1.Embed/Embed"),
                            ProstCodec::default(),
                        )
                        .await
                        .map_err(|error| {
                            EmbeddingError::ResponseError(format!("TEI gRPC: {error}"))
                        })?;
                    Ok::<_, EmbeddingError>(response.into_inner())
                })
                .await
                .map_err(|error| {
                    EmbeddingError::ResponseError(format!("TEI gRPC agotó el tiempo: {error}"))
                })??;
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
