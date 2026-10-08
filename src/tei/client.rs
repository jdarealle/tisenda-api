use super::{
    proto,
    rpc::{CONNECT_TIMEOUT, EMBED_DEADLINE, INFO_DEADLINE, tei_rpc},
    types::ModelIdentity,
};
use crate::Config;
use anyhow::{Context, Result, bail};
use tonic::transport::{Channel, Endpoint};

#[derive(Clone)]
pub(crate) struct TeiModel {
    channel: Channel,
    model: String,
    revision: Option<String>,
    embeddings: rig::DynModel<rig::operation::Embedding>,
}

impl TeiModel {
    pub(crate) const MAX_DOCUMENTS: usize = 16;

    pub(crate) fn embeddings(&self) -> rig::DynModel<rig::operation::Embedding> {
        self.embeddings.clone()
    }

    pub(crate) fn new(config: &Config) -> Result<Self> {
        let channel = Endpoint::from_shared(config.tei_url.trim_end_matches('/').to_string())
            .context("TEI_URL debe ser una URL gRPC válida")?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect_lazy();
        let embeddings = super::embedding::model(
            channel.clone(),
            &config.embedding_model,
            config.embedding_dimension,
        );
        Ok(Self {
            channel,
            embeddings,
            model: config.embedding_model.clone(),
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
        self.embeddings
            .embed_text("comprobación de dimensiones")
            .await?;
        Ok(identity)
    }
}
