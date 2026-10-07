use crate::logging::Failure;
use axum::{
    Router,
    extract::{MatchedPath, Request},
    middleware::{self, Next},
    response::Response,
};
use std::time::Duration;
use tower_http::{
    request_id::{PropagateRequestIdLayer, RequestId},
    trace::TraceLayer,
};
use tracing::Span;

pub(super) fn instrument(router: Router) -> Router {
    router
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| {
                    let id = request.extensions().get::<RequestId>()
                        .and_then(|id| id.header_value().to_str().ok()).unwrap_or("unavailable");
                    let route = request.extensions().get::<MatchedPath>()
                        .map_or("unmatched", MatchedPath::as_str);
                    // The application subscriber retains this context at WARN/ERROR too.
                    tracing::info_span!("http_request", request_id = id, method = %request.method(), route)
                })
                .on_request(())
                .on_response(|response: &Response, latency: Duration, span: &Span| {
                    let status = response.status().as_u16();
                    let request_id = response.headers().get("x-request-id")
                        .and_then(|value| value.to_str().ok());
                    let failure = response.extensions().get::<Failure>();
                    let health = response.extensions().get::<HealthCheck>().is_some();
                    macro_rules! completed {
                        ($level:ident) => {
                            tracing::$level!(parent: span,
                                event = "http_request_completed", request_id, status,
                                duration_ms = latency.as_secs_f64() * 1000.0,
                                error_code = failure.map(|failure| failure.code),
                                error_kind = failure.map(|failure| failure.kind),
                                upstream_http_status = failure.and_then(|failure| failure.http_status),
                                grpc_code = failure.and_then(|failure| failure.grpc_code),
                            )
                        };
                    }
                    match status {
                        500..=599 => completed!(error),
                        400..=499 => completed!(warn),
                        _ if health => completed!(debug),
                        _ => completed!(info),
                    }
                })
                // on_response already handles HTTP failures; don't duplicate them.
                .on_failure(())
                .on_body_chunk(())
                .on_eos(()),
        )
        .layer(middleware::from_fn(assign_request_id))
}

#[derive(Clone)]
pub(super) struct HealthCheck;

async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let value: axum::http::HeaderValue = uuid::Uuid::new_v4()
        .to_string()
        .parse()
        .expect("UUID is a valid header");
    request.headers_mut().insert("x-request-id", value.clone());
    request.extensions_mut().insert(RequestId::new(value));
    next.run(request).await
}
