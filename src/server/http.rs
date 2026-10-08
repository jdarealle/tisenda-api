use super::{ServerConfig, docs::RequestIdHeader};
use crate::{
    AnswerRequest, Config, answer,
    ingestions::{AcceptedBatch, BatchResult, Manager, Pagination, Selection, Worker},
    logging::Failure,
    qdrant,
};
use anyhow::Result;
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    info(title = "Tisenda API", description = "Ingesta de documentos y consultas con fuentes. Los cuerpos JSON admiten hasta 256 KiB."),
    paths(health, query, ingest_batch, get_batch),
    modifiers(&RequestIdHeader),
    tags(
        (name = "Salud", description = "Disponibilidad HTTP"),
        (name = "Consultas", description = "Respuestas con referencias documentales"),
        (name = "Ingesta", description = "Cola persistente y procesamiento secuencial de originales")
    )
)]
pub(super) struct ApiDoc;

#[derive(Serialize, ToSchema)]
struct HealthResponse {
    #[schema(examples("ok"))]
    status: &'static str,
}

#[derive(Serialize, ToSchema)]
struct ErrorResponse {
    /// Descripción del error de la solicitud.
    error: String,
}

/// Comprobar que la API responde.
///
/// No comprueba la disponibilidad de Docling, TEI, Qdrant ni OpenAI.
#[utoipa::path(
    get, path = "/health", tag = "Salud",
    responses((status = 200, description = "API disponible", body = HealthResponse))
)]
async fn health() -> Response {
    let mut response = Json(HealthResponse { status: "ok" }).into_response();
    response
        .extensions_mut()
        .insert(super::logging::HealthCheck);
    response
}

struct AppState {
    config: Config,
    ingestions: Arc<Manager>,
    worker: Arc<Worker>,
}

struct ApiError(StatusCode, String, Failure);

impl ApiError {
    fn new(status: StatusCode, message: String, code: &'static str) -> Self {
        Self(status, message, Failure::new(code))
    }

    fn caused(
        status: StatusCode,
        message: String,
        code: &'static str,
        error: &anyhow::Error,
    ) -> Self {
        Self(status, message, Failure::from_error(code, error))
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(ErrorResponse { error: self.1 })).into_response();
        response.extensions_mut().insert(self.2);
        response
    }
}

/// Initialize persistent storage and start the supervised ingestion worker.
/// Dropping the router cancels its worker; serve() additionally handles graceful shutdown.
pub async fn router(config: Config, server: ServerConfig) -> Result<Router> {
    Ok(build_router(config, server).await?.0)
}

async fn build_router(config: Config, server: ServerConfig) -> Result<(Router, Arc<Worker>)> {
    let ingestions = Arc::new(Manager::new(config.clone(), server).await?);
    let worker = Worker::start(ingestions.clone());
    let state = Arc::new(AppState {
        ingestions,
        config,
        worker: worker.clone(),
    });
    let app = super::logging::instrument(
        Router::new()
            .route("/health", get(health))
            .route("/query", post(query))
            .route("/ingestions", post(ingest_batch))
            .route("/ingestions/{batch_id}", get(get_batch))
            .layer(DefaultBodyLimit::max(256 * 1024))
            .with_state(state)
            .merge(super::docs::router()?),
    );
    Ok((app, worker))
}

pub async fn serve(config: Config, server: ServerConfig) -> Result<()> {
    let address = server.listen;
    let listener = tokio::net::TcpListener::bind(address).await?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let (app, worker) = build_router(config, server).await?;
    let stopping = worker.clone();
    tracing::info!(event = "server_listening", address = %listener.local_addr()?);
    let result = axum::serve(listener, app).with_graceful_shutdown(async move {
        tokio::select! {
            _ = async {
                #[cfg(unix)]
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                #[cfg(not(unix))]
                let _ = tokio::signal::ctrl_c().await;
            } => {},
            _ = stopping.wait_stopped() => {},
        }
        stopping.stop();
        tracing::info!(event = "server_stopping");
    });
    // Start the worker grace period at the signal, independently of open HTTP connections.
    let graceful_worker = worker.clone();
    let shutdown = async move {
        graceful_worker.wait_shutdown_requested().await;
        graceful_worker.shutdown().await
    };
    let serving = async {
        let result = result.await;
        worker.stop();
        result
    };
    let (http_result, worker_result) = tokio::join!(serving, shutdown);
    http_result?;
    worker_result?;
    tracing::info!(event = "server_stopped");
    Ok(())
}

/// Responder una pregunta con fuentes del índice.
///
/// `question` debe contener texto; los campos desconocidos se rechazan. El servidor
/// determina TOP_K. Las citas [n] se resuelven por sources[].id, no por posición.
/// Sin evidencia se devuelve 200 con sources vacío. El cuerpo admite hasta 256 KiB;
/// los rechazos JSON (incluido el exceso de tamaño) se convierten en 400.
#[utoipa::path(
    post, path = "/query", tag = "Consultas",
    request_body(content = AnswerRequest, example = json!({"question": "¿Qué consumible utiliza la impresora de recepción?"})),
    responses(
        (status = 200, description = "Respuesta con fuentes citadas, o sin evidencia suficiente", body = crate::Answer),
        (status = 400, description = "JSON inválido, cuerpo excesivo o pregunta vacía", body = ErrorResponse),
        (status = 500, description = "Configuración inválida", body = ErrorResponse),
        (status = 502, description = "Falló Qdrant, la generación o la validación de citas", body = ErrorResponse),
        (status = 503, description = "No hay índice activo", body = ErrorResponse)
    )
)]
async fn query(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<AnswerRequest>, JsonRejection>,
) -> Result<Json<crate::Answer>, ApiError> {
    let Json(request) = payload.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "JSON de consulta inválido".into(),
            "invalid_query_json",
        )
    })?;
    if request.question.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "La pregunta no puede estar vacía".into(),
            "empty_question",
        ));
    }
    let client = qdrant::client(&state.config).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuración inválida".into(),
            "qdrant_config_invalid",
        )
    })?;
    let active = qdrant::active_collection(&client, &state.config.qdrant_alias)
        .await
        .map_err(|error| {
            ApiError::caused(
                StatusCode::BAD_GATEWAY,
                "No se pudo consultar Qdrant".into(),
                "qdrant_unavailable",
                &error,
            )
        })?;
    if active.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "No hay índice activo".into(),
            "index_unavailable",
        ));
    }
    answer(&state.config, request)
        .await
        .map(Json)
        .map_err(query_error)
}

fn query_error(error: anyhow::Error) -> ApiError {
    let message = if let Some(error) = error.downcast_ref::<crate::answer::CitationError>() {
        error.to_string()
    } else {
        "No se pudo generar la respuesta".into()
    };
    let code = if error.is::<crate::answer::CitationError>() {
        "invalid_citations"
    } else {
        "answer_failed"
    };
    ApiError::caused(StatusCode::BAD_GATEWAY, message, code, &error)
}

/// Aceptar un lote después de persistir su selección.
///
/// {} o paths vacío selecciona toda DOCUMENTS_ROOT. Las rutas se normalizan,
/// ordenan y deduplican al aceptar; su contenido se lee cuando llega su turno.
/// Puede aceptar otros lotes mientras el worker procesa un documento.
#[utoipa::path(
    post, path = "/ingestions", tag = "Ingesta",
    request_body(content = Selection, examples(
        ("Toda la raíz" = (value = json!({}))),
        ("Selección" = (value = json!({"paths": ["manual.pdf", "catalogos"]})))
    )),
    responses(
        (status = 202, description = "Lote persistido; consultar Location", body = AcceptedBatch, headers(("Location" = String, description = "URL del estado del lote"))),
        (status = 400, description = "JSON o selección inválidos", body = ErrorResponse),
        (status = 500, description = "No se pudo persistir el lote", body = ErrorResponse),
        (status = 503, description = "Worker detenido", body = ErrorResponse)
    )
)]
async fn ingest_batch(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<Selection>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(selection) = payload.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "Selección JSON inválida".into(),
            "invalid_selection_json",
        )
    })?;
    if !state.worker.running() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Worker detenido".into(),
            "worker_stopped",
        ));
    }
    let keys = state.ingestions.select(selection).await.map_err(|e| {
        ApiError::caused(
            StatusCode::BAD_REQUEST,
            format!("{e:#}"),
            "invalid_selection",
            &e,
        )
    })?;
    let accepted = state.ingestions.enqueue(keys).await.map_err(|e| {
        state.worker.stop();
        ApiError::caused(
            StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo persistir el lote".into(),
            "ingestion_store_failed",
            &e,
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, accepted.status_url.clone())],
        Json(accepted),
    )
        .into_response())
}

/// Consultar progreso y documentos en orden de selección, con una página de hasta 500.
/// Los contadores abarcan todo el lote; completed indica que todos sus trabajos son terminales.
#[utoipa::path(
    get, path = "/ingestions/{batch_id}", tag = "Ingesta",
    params(("batch_id" = String, Path, description = "Identificador del lote"), Pagination),
    responses(
        (status = 200, description = "Progreso y página de documentos", body = BatchResult),
        (status = 400, description = "Paginación inválida", body = ErrorResponse),
        (status = 404, description = "Lote inexistente", body = ErrorResponse),
        (status = 500, description = "No se pudo consultar SQLite", body = ErrorResponse)
    )
)]
async fn get_batch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    page: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<BatchResult>, ApiError> {
    let Query(page) = page.map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "Paginación inválida".into(),
            "invalid_pagination",
        )
    })?;
    page.validate()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string(), "invalid_pagination"))?;
    state
        .ingestions
        .batch(&id, &page)
        .await
        .map_err(|e| {
            ApiError::caused(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo consultar el lote".into(),
                "ingestion_store_failed",
                &e,
            )
        })?
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "Lote inexistente".into(),
                "batch_not_found",
            )
        })
}
