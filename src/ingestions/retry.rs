use anyhow::Error;
use qdrant_client::QdrantError;

pub(super) fn transient(error: &Error) -> bool {
    error.chain().any(cause_transient)
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
    if let Some(rig::embeddings::EmbeddingError::DocumentError(e)) =
        cause.downcast_ref::<rig::embeddings::EmbeddingError>()
    {
        let mut current = Some(e.as_ref() as &(dyn std::error::Error + 'static));
        while let Some(e) = current {
            if cause_transient(e) {
                return true;
            }
            current = e.source();
        }
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
