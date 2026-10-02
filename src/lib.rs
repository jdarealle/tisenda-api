mod answer;
mod config;
mod docling;
mod documents;
mod ingest;
mod qdrant;
mod server;
mod tei;

pub use answer::{Answer, AnswerRequest, Source, answer};
pub use config::Config;
pub use documents::DoclingChunk;
pub use ingest::{DocumentIngestResult, IngestDocument, IngestReport, ingest_documents};
pub use server::{ServerConfig, router, serve};
