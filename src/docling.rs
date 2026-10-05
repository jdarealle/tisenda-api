//! Conversion and chunk validation through Docling Serve.

mod archive;
mod client;
mod formats;
mod pipeline;
mod provenance;
mod types;

pub(crate) use client::Client;
pub(crate) use formats::input_format;
pub(crate) use pipeline::process_original;
pub(crate) use types::{ArchiveLimits, FileReport};
