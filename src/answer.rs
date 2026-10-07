//! Grounded answers with explicit, validated references to indexed fragments.

mod citations;
mod context;
mod generation;
mod pipeline;
mod types;

pub use pipeline::answer;
pub(crate) use types::CitationError;
pub use types::{Answer, AnswerRequest, Source, SourceLocation};
