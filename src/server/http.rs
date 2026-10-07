use super::ServerConfig;
use crate::{
    AnswerRequest, Config, answer,
    ingestions::{BatchResult, Manager, Selection},
    qdrant,
};
use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    info(title = "Tisenda API", description = "Ingesta de documentos y consultas con fuentes. Los cuerpos JSON admiten hasta 256 KiB."),
    paths(health, query, ingest_batch),
    tags(
        (name = "Salud", description = "Disponibilidad HTTP"),
        (name = "Consultas", description = "Respuestas con referencias documentales"),
        (name = "Ingesta", description = "Procesamiento síncrono de originales")
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
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

struct AppState {
    config: Config,
    ingestions: Arc<Manager>,
}

struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorResponse { error: self.1 })).into_response()
    }
}

pub fn router(config: Config, mut server: ServerConfig) -> Result<Router> {
    server.validate(&config)?;
    let state = Arc::new(AppState {
        ingestions: Arc::new(Manager::new(config.clone(), server)?),
        config,
    });
    Ok(Router::new()
        .route("/health", get(health))
        .route("/query", post(query))
        .route("/ingestions", post(ingest_batch))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
        .merge(super::docs::router()?))
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
    let Json(request) = payload
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "JSON de consulta inválido".into()))?;
    if request.question.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "La pregunta no puede estar vacía".into(),
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
    ApiError(StatusCode::BAD_GATEWAY, message)
}

/// Procesar una selección de documentos hasta terminar el lote.
///
/// {} o paths vacío selecciona toda la raíz. Las rutas son relativas a DOCUMENTS_ROOT;
/// se rechazan rutas ocultas, absolutas, con .. o enlaces simbólicos. La selección
/// se ordena y deduplica. Solo se admite un lote activo por proceso.
/// La conexión permanece abierta durante el procesamiento: un 200 puede incluir
/// archivos rechazados o fallidos. El cuerpo admite hasta 256 KiB; los rechazos
/// JSON (incluido el exceso de tamaño) se convierten en 400.
#[utoipa::path(
    post, path = "/ingestions", tag = "Ingesta",
    request_body(content = Selection, examples(
        ("Toda la raíz" = (value = json!({}))),
        ("Selección" = (value = json!({"paths": ["manual.pdf", "catalogos"]})))
    )),
    responses(
        (status = 200, description = "Lote terminado; revisar los resultados individuales. Una selección vacía devuelve contadores en cero", body = BatchResult),
        (status = 400, description = "JSON, tamaño del cuerpo o selección inválidos", body = ErrorResponse),
        (status = 409, description = "Esquema del índice incompatible", body = ErrorResponse),
        (status = 502, description = "Falló la comprobación del índice", body = ErrorResponse),
        (status = 503, description = "Ya existe un lote activo", body = ErrorResponse)
    )
)]
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
