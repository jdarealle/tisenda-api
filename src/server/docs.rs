use super::http::ApiDoc;
use anyhow::Result;
use axum::{Router, http::header, response::Html, routing::get};
use scalar_api_reference::scalar_html_default;
use serde_json::json;
use utoipa::OpenApi;

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
