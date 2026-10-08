use anyhow::Error;
use qdrant_client::QdrantError;

pub(super) fn transient(error: &Error) -> bool {
    crate::logging::causes(error.as_ref()).any(cause_transient)
}
fn cause_transient(cause: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(e) = cause.downcast_ref::<crate::logging::HttpFailure>() {
        return http(e.status());
    }
    if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
        return e.status().is_some_and(|s| http(s.as_u16()))
            || e.is_timeout()
            || e.is_connect()
            || e.is_body();
    }
    if let Some(e) = cause.downcast_ref::<tonic::Status>() {
        return grpc(e.code());
    }
    if let Some(
        QdrantError::ResponseError { status } | QdrantError::ResourceExhaustedError { status, .. },
    ) = cause.downcast_ref::<QdrantError>()
    {
        return grpc(status.code());
    }
    if let Some(error) = cause.downcast_ref::<rig::ProviderError>() {
        if let rig::ProviderError::Http(transport) = error {
            // Rig treats every Instance as transient. A preserved gRPC or reqwest
            // error carries a more precise verdict, including permanent failures.
            for source in crate::logging::causes(transport.as_ref()) {
                if let Some(status) = source.downcast_ref::<tonic::Status>() {
                    return grpc(status.code());
                }
                if let Some(error) = source.downcast_ref::<reqwest::Error>() {
                    return cause_transient(error);
                }
            }
        }
        return error.is_retryable();
    }
    cause.is::<tokio::time::error::Elapsed>() || cause.is::<ConversionTimeout>()
}
fn http(status: u16) -> bool {
    matches!(status, 408 | 429 | 500..=599)
}
fn grpc(code: tonic::Code) -> bool {
    matches!(
        code,
        tonic::Code::Unavailable
            | tonic::Code::DeadlineExceeded
            | tonic::Code::ResourceExhausted
            | tonic::Code::Aborted
            | tonic::Code::Internal
            | tonic::Code::Unknown
    )
}
pub(super) fn delay(attempt: i64) -> Option<i64> {
    match attempt {
        1 => Some(10),
        2 => Some(60),
        _ => None,
    }
}
#[derive(Debug)]
pub(crate) struct ConversionTimeout(pub(crate) String);
impl std::fmt::Display for ConversionTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Venció el tiempo de conversión; tarea Docling {} puede continuar en el servicio",
            self.0
        )
    }
}
impl std::error::Error for ConversionTimeout {}
