//! Selección y procesamiento de lotes desde la raíz documental.

mod manager;
mod source;
mod types;

pub(crate) use manager::Manager;
pub(crate) use types::{BatchResult, Selection};
