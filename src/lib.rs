mod answer;
mod config;
mod docling;
mod documents;
mod ingest;
mod ingestions;
mod logging;
mod qdrant;
mod server;
mod tei;

pub use answer::{Answer, AnswerRequest, Source, SourceLocation, answer};
pub use config::Config;
pub use documents::DoclingChunk;
pub use ingest::{DocumentIngestResult, IngestDocument, IngestReport, ingest_documents};
pub use logging::{LoggingGuard, init_logging};
pub use server::{ServerConfig, router, serve};
