use crate::{
    AnswerRequest, Config, answer,
    docling::{self, ArchiveLimits, Callback, Progress},
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
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub docling_url: reqwest::Url,
    pub max_download_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_archive_entries: usize,
    pub request_timeout_secs: u64,
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
        })
    }
}

struct AppState {
    config: Config,
    server: ServerConfig,
    http: reqwest::Client,
    // Local mutual exclusion only: no registry, durable queue or retry state.
    ingest: Arc<tokio::sync::Mutex<()>>,
}

struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

pub fn router(config: Config, mut server: ServerConfig) -> Result<Router> {
    if !matches!(server.docling_url.scheme(), "http" | "https")
        || server.docling_url.host_str().is_none()
        || server.docling_url.query().is_some()
        || server.docling_url.fragment().is_some()
        || !server.docling_url.username().is_empty()
        || server.docling_url.password().is_some()
    {
        bail!("DOCLING_URL debe ser una URL HTTP válida sin credenciales, query ni fragmento");
    }
    if !server.docling_url.path().ends_with('/') {
        server
            .docling_url
            .set_path(&format!("{}/", server.docling_url.path()));
    }
    if server.max_download_bytes == 0
        || server.max_expanded_bytes == 0
        || server.max_archive_entries == 0
        || server.request_timeout_secs == 0
        || server.max_expanded_bytes == u64::MAX
        || config.max_file_bytes == u64::MAX
    {
        bail!("Límites HTTP/ZIP inválidos");
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(server.request_timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let state = Arc::new(AppState {
        config,
        server,
        http,
        ingest: Arc::new(tokio::sync::Mutex::new(())),
    });
    Ok(Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/query", post(query))
        .route("/webhooks/docling", post(callback))
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
    let Json(request) =
        payload.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "JSON de consulta inválido"))?;
    if request.question.trim().is_empty()
        || !(1..=50).contains(&request.top_k.unwrap_or(state.config.top_k))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Pregunta vacía o top_k fuera de 1..50",
        ));
    }
    let client = qdrant::client(&state.config)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Configuración inválida"))?;
    let active = qdrant::active_collection(&client, &state.config.qdrant_alias)
        .await
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "No se pudo consultar Qdrant"))?;
    if active.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "No hay índice activo",
        ));
    }
    answer(&state.config, request)
        .await
        .map(Json)
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "No se pudo generar la respuesta"))
}

async fn callback(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<Callback>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(callback) =
        payload.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Callback inválido"))?;
    uuid::Uuid::parse_str(&callback.task_id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "task_id inválido"))?;
    if let Progress::UpdateProcessed(batch) = callback.progress {
        batch
            .validate()
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "Resumen de conversión inválido"))?;
        let task_id = callback.task_id;
        match state.ingest.clone().try_lock_owned() {
            Ok(guard) => {
                tokio::spawn(async move {
                    let _guard = guard;
                    let limits = ArchiveLimits {
                        download: state.server.max_download_bytes,
                        expanded: state.server.max_expanded_bytes,
                        file: state.config.max_file_bytes,
                        entries: state.server.max_archive_entries,
                    };
                    docling::process(
                        &state.config,
                        &state.http,
                        &state.server.docling_url,
                        limits,
                        &task_id,
                        batch,
                    )
                    .await;
                });
            }
            Err(_) => docling::log_result(&task_id, None, false),
        }
    }
    Ok(Json(json!({"status": "ack"})))
}
