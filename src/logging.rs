//! Configuración de salida y diagnóstico estructurado sin datos de negocio.

mod config;
mod diagnostic;
mod output;
mod runtime;

pub(crate) use diagnostic::{Failure, HttpFailure, causes, operation};
pub use runtime::{LoggingGuard, init_logging};
