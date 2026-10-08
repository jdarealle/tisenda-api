use anyhow::{Context, Result};
use std::{future::Future, time::Duration};
use tonic::{Request, Response, Status};

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const INFO_DEADLINE: Duration = Duration::from_secs(15);
pub(super) const EMBED_DEADLINE: Duration = Duration::from_secs(120);
// Cubre también la espera del cliente y la recepción del cuerpo. Un vencimiento
// aquí se informa como protección local, separado del estado devuelto por gRPC.
const LOCAL_TIMEOUT_GRACE: Duration = Duration::from_secs(10);

pub(super) async fn tei_rpc<M, T, F>(
    rpc: &'static str,
    deadline: Duration,
    message: M,
    call: impl FnOnce(Request<M>) -> F,
) -> Result<T>
where
    F: Future<Output = Result<Response<T>, Status>>,
{
    crate::logging::operation("tei", rpc, async {
        let mut request = Request::new(message);
        request.set_timeout(deadline);
        let local_timeout = deadline + LOCAL_TIMEOUT_GRACE;
        let response = tokio::time::timeout(local_timeout, call(request))
            .await
            .with_context(|| {
                format!(
                    "TEI {rpc}: venció la protección local de {} s (deadline gRPC de {} s)",
                    local_timeout.as_secs(),
                    deadline.as_secs()
                )
            })?
            .with_context(|| format!("TEI gRPC {rpc} devolvió un error"))?;
        Ok(response.into_inner())
    })
    .await
}
