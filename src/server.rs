use crate::{
    AnswerRequest, Config, answer,
    ingestions::{BatchResult, Manager, Selection},
    qdrant,
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub docling_url: reqwest::Url,
    pub max_download_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_archive_entries: usize,
    pub request_timeout_secs: u64,
    pub conversion_timeout_secs: u64,
    pub image_export_mode: String,
    pub do_ocr: bool,
    pub ocr_preset: String,
    pub documents_root: PathBuf,
    pub max_original_bytes: u64,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        fn value(name: &str, default: &str) -> String {
            std::env::var(name).unwrap_or_else(|_| default.to_string())
        }
        Ok(Self {
            listen: value("HTTP_BIND", "0.0.0.0:3000")
                .parse()
                .context("HTTP_BIND inválido")?,
            docling_url: value("DOCLING_URL", "http://127.0.0.1:5001/")
                .parse()
                .context("DOCLING_URL inválida")?,
            max_download_bytes: value("DOCLING_MAX_DOWNLOAD_BYTES", "33554432").parse()?,
            max_expanded_bytes: value("DOCLING_MAX_EXPANDED_BYTES", "134217728").parse()?,
            max_archive_entries: value("DOCLING_MAX_ARCHIVE_ENTRIES", "200").parse()?,
            request_timeout_secs: value("DOCLING_REQUEST_TIMEOUT_SECS", "120").parse()?,
            conversion_timeout_secs: value("DOCLING_CONVERSION_TIMEOUT_SECS", "1800").parse()?,
            image_export_mode: value("DOCLING_IMAGE_EXPORT_MODE", "referenced"),
            do_ocr: value("DOCLING_DO_OCR", "true")
                .parse()
                .context("DOCLING_DO_OCR debe ser true o false")?,
            ocr_preset: value("DOCLING_OCR_PRESET", "auto"),
            documents_root: value("DOCUMENTS_ROOT", "manuales").into(),
            max_original_bytes: value("DOCLING_SERVE_MAX_FILE_SIZE", "52428800").parse()?,
        })
    }
    pub(crate) fn validate(&mut self, config: &Config) -> Result<()> {
        if !matches!(self.docling_url.scheme(), "http" | "https")
            || self.docling_url.host_str().is_none()
            || self.docling_url.query().is_some()
            || self.docling_url.fragment().is_some()
            || !self.docling_url.username().is_empty()
            || self.docling_url.password().is_some()
        {
            bail!("DOCLING_URL debe ser una URL HTTP válida sin credenciales, query ni fragmento");
        }
        if !self.docling_url.path().ends_with('/') {
            self.docling_url
                .set_path(&format!("{}/", self.docling_url.path()));
        }
        if self.max_download_bytes == 0
            || self.max_expanded_bytes == 0
            || self.max_archive_entries == 0
            || self.request_timeout_secs == 0
            || self.max_expanded_bytes == u64::MAX
            || config.max_file_bytes == u64::MAX
            || self.conversion_timeout_secs == 0
            || self.max_original_bytes == 0
        {
            bail!("Límites HTTP/ZIP inválidos");
        }
        if !matches!(
            self.image_export_mode.as_str(),
            "referenced" | "embedded" | "placeholder"
        ) {
            bail!("DOCLING_IMAGE_EXPORT_MODE debe ser referenced, embedded o placeholder");
        }
        if self.do_ocr && self.ocr_preset.trim().is_empty() {
            bail!("DOCLING_OCR_PRESET no puede estar vacío cuando DOCLING_DO_OCR=true");
        }
        Ok(())
    }
}

struct AppState {
    config: Config,
    ingestions: Arc<Manager>,
}

struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

pub fn router(config: Config, mut server: ServerConfig) -> Result<Router> {
    server.validate(&config)?;
    let state = Arc::new(AppState {
        ingestions: Arc::new(Manager::new(config.clone(), server)?),
        config,
    });
    Ok(Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/query", post(query))
        .route("/ingestions", post(ingest_batch))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state))
}

pub async fn serve(config: Config, server: ServerConfig) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(server.listen).await?;
    axum::serve(listener, router(config, server)?)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn query(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<AnswerRequest>, JsonRejection>,
) -> Result<Json<crate::Answer>, ApiError> {
    let Json(request) = payload
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "JSON de consulta inválido".into()))?;
    if request.question.trim().is_empty()
        || !(1..=50).contains(&request.top_k.unwrap_or(state.config.top_k))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Pregunta vacía o top_k fuera de 1..50".into(),
        ));
    }
    let client = qdrant::client(&state.config).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuración inválida".into(),
        )
    })?;
    let active = qdrant::active_collection(&client, &state.config.qdrant_alias)
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "No se pudo consultar Qdrant".into(),
            )
        })?;
    if active.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "No hay índice activo".into(),
        ));
    }
    answer(&state.config, request).await.map(Json).map_err(|_| {
        ApiError(
            StatusCode::BAD_GATEWAY,
            "No se pudo generar la respuesta".into(),
        )
    })
}

async fn ingest_batch(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<Selection>, JsonRejection>,
) -> Result<Json<BatchResult>, ApiError> {
    let Json(selection) =
        payload.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Selección JSON inválida".into()))?;
    let _permit = state
        .ingestions
        .reserve()
        .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    let keys = state
        .ingestions
        .select(selection)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    // Rejected-only batches need no conversion or vector service access.
    if keys
        .iter()
        .any(|key| crate::docling::input_format(key).is_some())
    {
        let incompatibility = state
            .ingestions
            .check_index()
            .await
            .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
        if let Some(message) = incompatibility {
            return Err(ApiError(StatusCode::CONFLICT, message));
        }
    }
    Ok(Json(state.ingestions.run(keys).await))
}
