use anyhow::Result;
use std::{future::Future, time::Instant};
use tracing::Instrument;

/// Conserva el error HTTP existente para el llamador y expone su estado al logger.
/// El mensaje nunca se copia a Failure.
pub(crate) struct HttpFailure {
    status: u16,
    message: String,
}

impl HttpFailure {
    pub(crate) fn new(status: u16, message: String) -> Self {
        Self { status, message }
    }
}

impl std::fmt::Debug for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpFailure")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpFailure {}

/// Solo campos controlados: nunca formatea el mensaje ni la cadena del error.
#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub(crate) code: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) http_status: Option<u16>,
    pub(crate) grpc_code: Option<i32>,
    pub(crate) os_error_code: Option<i32>,
}

impl Failure {
    pub(crate) fn new(code: &'static str) -> Self {
        Self {
            code,
            kind: "application",
            http_status: None,
            grpc_code: None,
            os_error_code: None,
        }
    }

    pub(crate) fn from_error(code: &'static str, error: &anyhow::Error) -> Self {
        let mut failure = Self::new(code);
        for cause in error.chain() {
            if let Some(error) = cause.downcast_ref::<HttpFailure>() {
                failure.kind = "http";
                failure.http_status = Some(error.status);
                break;
            }
            if let Some(error) = cause.downcast_ref::<reqwest::Error>() {
                failure.kind = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connection"
                } else {
                    "http"
                };
                failure.http_status = error.status().map(|status| status.as_u16());
                break;
            }
            if let Some(error) = cause.downcast_ref::<tonic::Status>() {
                failure.kind = "grpc";
                failure.grpc_code = Some(error.code() as i32);
                break;
            }
            if cause.is::<tokio::time::error::Elapsed>() {
                failure.kind = "timeout";
                break;
            }
            if let Some(error) = cause.downcast_ref::<std::io::Error>() {
                failure.kind = "io";
                failure.os_error_code = error.raw_os_error();
                break;
            }
        }
        failure
    }
}

/// Mide una operación sin cambiar su resultado ni imprimir argumentos o errores.
pub(crate) async fn operation<T>(
    service: &'static str,
    operation: &'static str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    async {
        let started = Instant::now();
        let result = future.await;
        let failure = result
            .as_ref()
            .err()
            .map(|error| Failure::from_error("operation_failed", error));
        if let Some(failure) = failure {
            tracing::warn!(
                event = "operation_failed",
                service,
                operation,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                error_kind = failure.kind,
                upstream_http_status = failure.http_status,
                grpc_code = failure.grpc_code,
                os_error_code = failure.os_error_code,
            );
        } else {
            tracing::debug!(
                event = "operation_completed",
                service,
                operation,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                outcome = "success",
            );
        }
        result
    }
    .instrument(tracing::debug_span!("dependency", service, operation).or_current())
    .await
}
