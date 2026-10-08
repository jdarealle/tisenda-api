use super::{
    client::TeiModel,
    proto,
    rpc::{EMBED_DEADLINE, tei_rpc},
};
use futures_util::future::try_join_all;
use rig::{
    DynModel, Model, ProviderError,
    driver::{Exchange, Local, Opened, Opening, Step, Transport},
    embeddings::{Embedding, EmbeddingResponse},
    operation::Embedding as EmbeddingOp,
    wire::Capabilities,
};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tonic::transport::Channel;

pub(super) fn model(channel: Channel, model: &str, dimension: usize) -> DynModel<EmbeddingOp> {
    Model::new(
        Local::<EmbeddingOp>::new("tei")
            .with_id(model)
            .with_capabilities(
                Capabilities::embedding(TeiModel::MAX_DOCUMENTS, dimension)
                    .declaring(Some(dimension)),
            ),
        TeiTransport {
            channel,
            permits: Arc::new(Semaphore::new(TeiModel::MAX_DOCUMENTS)),
        },
    )
    .erase()
}

#[derive(Clone)]
struct TeiTransport {
    channel: Channel,
    // Shared by cloned models, including concurrently scheduled builder batches.
    permits: Arc<Semaphore>,
}

impl Transport<Local<EmbeddingOp>> for TeiTransport {
    fn send(&self, texts: Vec<String>, _exchange: Exchange) -> Opening<Step<EmbeddingOp>> {
        let transport = self.clone();
        Opening::new(async move {
            let requests = texts.into_iter().map(|text| {
                let transport = transport.clone();
                async move {
                    let _permit = transport
                        .permits
                        .acquire()
                        .await
                        .expect("the private TEI semaphore is never closed");
                    let mut client = proto::embed_client::EmbedClient::new(transport.channel);
                    let response = tei_rpc(
                        "Embed",
                        EMBED_DEADLINE,
                        proto::EmbedRequest {
                            inputs: text,
                            truncate: false,
                            normalize: Some(true),
                        },
                        move |request| async move { client.embed(request).await },
                    )
                    .await
                    .map_err(embedding_error)?;
                    let vec: Vec<f64> = response.embeddings.into_iter().map(f64::from).collect();
                    if vec.iter().any(|value| !value.is_finite()) {
                        return Err(ProviderError::Response(
                            "TEI devolvió un vector inválido; se esperaban valores finitos".into(),
                        ));
                    }
                    // Rig pairs vectors with the original inputs and checks the declared width.
                    Ok(Embedding {
                        document: String::new(),
                        vec,
                    })
                }
            });
            let embeddings = try_join_all(requests).await?;
            Ok(Opened::new(futures_util::stream::once(async move {
                Ok(Step::End(EmbeddingResponse::new(embeddings)))
            })))
        })
    }
}

/// Retain typed causes through Rig's shared transport error for retry and diagnostics.
#[derive(Debug)]
struct EmbeddingFailure(anyhow::Error);

impl std::fmt::Display for EmbeddingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for EmbeddingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn embedding_error(error: anyhow::Error) -> ProviderError {
    ProviderError::from_transport_error(rig::http_client::Error::instance(EmbeddingFailure(error)))
}
