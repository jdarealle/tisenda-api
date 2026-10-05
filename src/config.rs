use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub(crate) const EMBEDDING_PREPROCESSING: &str = "bge-m3-dense-grpc-normalized-v1";

#[derive(Clone)]
pub struct Config {
    pub qdrant_url: String,
    pub qdrant_alias: String,
    pub tei_url: String,
    pub embedding_model: String,
    pub embedding_revision: Option<String>,
    pub embedding_dimension: usize,
    pub openai_model: String,
    pub openai_api_key: Option<String>,
    pub chunk_target_tokens: usize,
    pub max_file_bytes: u64,
    pub top_k: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        fn value(key: &str, default: &str) -> String {
            std::env::var(key).unwrap_or_else(|_| default.to_string())
        }
        fn number<T: std::str::FromStr>(key: &str, default: &str) -> Result<T>
        where
            T::Err: std::error::Error + Send + Sync + 'static,
        {
            value(key, default)
                .parse()
                .with_context(|| format!("{key} debe ser un número válido"))
        }
        let config = Self {
            qdrant_url: value("QDRANT_URL", "http://127.0.0.1:6334"),
            qdrant_alias: value("QDRANT_ALIAS", "rag_docling"),
            tei_url: value("TEI_URL", "http://127.0.0.1:8080"),
            embedding_model: value("EMBEDDING_MODEL", "BAAI/bge-m3"),
            embedding_revision: std::env::var("EMBEDDING_REVISION")
                .ok()
                .filter(|revision| !revision.trim().is_empty()),
            embedding_dimension: number("EMBEDDING_DIMENSION", "1024")?,
            openai_model: value("OPENAI_MODEL", "gpt-5-mini"),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            chunk_target_tokens: {
                if std::env::var_os("CHUNK_MAX_TOKENS").is_some() {
                    tracing::warn!(
                        "CHUNK_MAX_TOKENS está obsoleto; usa CHUNK_TARGET_TOKENS (objetivo, no límite de aceptación)"
                    );
                }
                let legacy = std::env::var("CHUNK_MAX_TOKENS").unwrap_or_else(|_| "512".into());
                number("CHUNK_TARGET_TOKENS", &legacy)?
            },
            max_file_bytes: number("MAX_FILE_BYTES", "10485760")?,
            top_k: number("TOP_K", "5")?,
        };
        if config.embedding_dimension == 0
            || config.chunk_target_tokens == 0
            || config.max_file_bytes == 0
            || config.top_k == 0
            || config.top_k > 50
        {
            bail!(
                "Configuración numérica inválida: comprueba dimensión, límite de tokens, límite de archivo y TOP_K"
            );
        }
        if config.qdrant_alias.is_empty() {
            bail!("QDRANT_ALIAS inválido");
        }
        Ok(config)
    }

    pub(crate) fn embedding_version(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.embedding_model.as_bytes());
        hasher.update(self.embedding_dimension.to_le_bytes());
        hasher.update(EMBEDDING_PREPROCESSING.as_bytes());
        format!("{:x}", hasher.finalize())[..12].to_string()
    }
}
