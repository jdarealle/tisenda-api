//! Configuración, servicio HTTP y documentación de la API.

mod config;
mod docs;
mod http;
mod logging;

pub use config::ServerConfig;
pub use http::{router, serve};
