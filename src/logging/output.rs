use super::config::Mode;
use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter,
    filter::{FilterExt, filter_fn},
    fmt::MakeWriter,
    prelude::*,
};

pub(super) fn subscriber<W>(
    mode: Mode,
    filter: EnvFilter,
    writer: W,
    ansi: bool,
) -> Box<dyn Subscriber + Send + Sync>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    // Keep application context even when RUST_LOG suppresses INFO/DEBUG events.
    // This exception enables spans only; even with retained context, `off` emits nothing.
    let filter = filter.or(filter_fn(|metadata| {
        metadata.is_span() && (metadata.target() == "rag" || metadata.target().starts_with("rag::"))
    }));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_target(true);
    match mode {
        Mode::Development => Box::new(
            tracing_subscriber::registry()
                .with(layer.compact().with_ansi(ansi).with_filter(filter)),
        ),
        Mode::Production => Box::new(
            tracing_subscriber::registry().with(layer.json().with_ansi(false).with_filter(filter)),
        ),
    }
}
