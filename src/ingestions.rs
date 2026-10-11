//! Catálogo documental, cola persistente, selección segura y worker de ingesta.

mod catalog;
mod manager;
mod progress;
mod retry;
mod source;
mod store;
mod types;
mod worker;

pub(crate) use manager::Manager;
pub(crate) use progress::Progress;
pub(crate) use retry::ConversionTimeout;
pub(crate) use types::{AcceptedBatch, BatchResult, Pagination, Selection};
pub(crate) use worker::Worker;
