use super::{config::Mode, output};
use anyhow::{Context, Result};
use std::{io::IsTerminal, time::Duration};
use tokio::task::JoinHandle;
use tracing_appender::non_blocking::{ErrorCounter, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::util::SubscriberInitExt;

/// Mantiene vivo el escritor. Conservar hasta después del cierre del servidor.
/// La destrucción intenta vaciar la cola; no garantiza entrega ante una salida bloqueada.
pub struct LoggingGuard {
    monitor: JoinHandle<()>,
    counter: ErrorCounter,
    _worker: WorkerGuard,
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        self.monitor.abort();
        let dropped_lines = self.counter.dropped_lines();
        if dropped_lines > 0 {
            tracing::warn!(
                event = "logs_dropped",
                dropped_lines,
                "Log queue discarded events"
            );
        }
    }
}

/// Instala el subscriber global con el filtro RUST_LOG y formato según debug_assertions.
/// Requiere un runtime Tokio activo; la biblioteca no lo invoca automáticamente.
pub fn init_logging() -> Result<LoggingGuard> {
    fn value(name: &str) -> Result<Option<String>> {
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => anyhow::bail!("{name} debe contener texto UTF-8"),
        }
    }
    let runtime = tokio::runtime::Handle::try_current().context("Logging requiere Tokio")?;
    let mode = Mode::for_build();
    let filter = mode.filter(value("RUST_LOG")?.as_deref())?;
    let ansi = mode.ansi(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()),
    );
    let (writer, worker) = NonBlockingBuilder::default()
        .buffered_lines_limit(8192)
        .lossy(true)
        .finish(std::io::stdout());
    let counter = writer.error_counter();
    output::subscriber(mode, filter, writer, ansi)
        .try_init()
        .map_err(|_| anyhow::anyhow!("No se pudo instalar el subscriber global"))?;
    let monitor_counter = counter.clone();
    let monitor = runtime.spawn(async move {
        let mut previous = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let dropped_lines = monitor_counter.dropped_lines();
            if dropped_lines > previous {
                tracing::warn!(
                    event = "logs_dropped",
                    dropped_lines,
                    "Log queue discarded events"
                );
                previous = dropped_lines;
            }
        }
    });
    Ok(LoggingGuard {
        monitor,
        counter,
        _worker: worker,
    })
}
