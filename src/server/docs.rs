use super::http::ApiDoc;
use anyhow::Result;
use axum::{Router, http::header, response::Html, routing::get};
use scalar_api_reference::scalar_html_default;
use serde_json::json;
use utoipa::OpenApi;

/// Describe la cabecera generada por el middleware en todas las respuestas de la API.
pub(super) struct RequestIdHeader;

impl utoipa::Modify for RequestIdHeader {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{
            RefOr,
            header::HeaderBuilder,
            schema::{ObjectBuilder, SchemaFormat, Type},
        };
        let header = HeaderBuilder::new()
            .description(Some("UUID generado por la API para correlacionar la solicitud con sus logs. Reemplaza cualquier identificador enviado por el cliente."))
            .schema(ObjectBuilder::new().schema_type(Type::String)
                .format(Some(SchemaFormat::Custom("uuid".into()))))
            .build();
        for path in openapi.paths.paths.values_mut() {
            for operation in [
                &mut path.get,
                &mut path.put,
                &mut path.post,
                &mut path.delete,
                &mut path.options,
                &mut path.head,
                &mut path.patch,
                &mut path.trace,
            ]
            .into_iter()
            .flatten()
            {
                for response in operation.responses.responses.values_mut() {
                    if let RefOr::T(response) = response {
                        response
                            .headers
                            .insert("x-request-id".into(), header.clone());
                    }
                }
            }
        }
    }
}

/// Build the specification and HTML once, without accessing external services.
pub(super) fn router() -> Result<Router> {
    let specification = ApiDoc::openapi().to_json()?;
    let html = scalar_html_default(&json!({
        "url": "/openapi.json",
        "agent": { "disabled": true }
    }));
    Ok(Router::new()
        .route("/docs", get(move || std::future::ready(Html(html.clone()))))
        .route(
            "/openapi.json",
            get(move || {
                std::future::ready((
                    [(header::CONTENT_TYPE, "application/json")],
                    specification.clone(),
                ))
            }),
        ))
}
