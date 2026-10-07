use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

pub(super) const NO_EVIDENCE: &str =
    "No hay información suficiente en los documentos indexados para responder esa pregunta.";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerRequest {
    pub question: String,
}

/// A fragment cited in this response; `id` is local to the response, not the index.
#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub id: String,
    pub filename: String,
    pub source_key: String,
    pub location: SourceLocation,
    /// Exact indexed text supplied to the generator, including structural context.
    pub excerpt: String,
}

/// Available locators only; the format category does not imply precise coordinates.
#[derive(Clone, Debug, Serialize)]
pub struct SourceLocation {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_numbers: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headings: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Answer {
    pub text: String,
    pub sources: Vec<Source>,
}

impl Answer {
    pub(super) fn no_evidence() -> Self {
        Self {
            text: NO_EVIDENCE.into(),
            sources: Vec::new(),
        }
    }
}

/// Both generation attempts failed structural citation validation.
#[derive(Debug)]
pub(crate) struct CitationError;

impl fmt::Display for CitationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("No se pudo generar una respuesta con referencias válidas")
    }
}

impl Error for CitationError {}
