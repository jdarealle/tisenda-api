use super::{Answer, Source, types::NO_EVIDENCE};
use std::collections::{HashMap, HashSet};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CitationIssue {
    EmptyResponse,
    MissingCitation,
    InvalidMarker,
    UnknownSource,
}

impl CitationIssue {
    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::EmptyResponse => "empty_response",
            Self::MissingCitation => "missing_citation",
            Self::InvalidMarker => "invalid_marker",
            Self::UnknownSource => "unknown_source",
        }
    }

    pub(super) fn correction(&self) -> &'static str {
        match self {
            Self::EmptyResponse => "La respuesta está vacía; proporciona una respuesta con citas.",
            Self::MissingCitation => "La respuesta informativa no contiene ninguna cita.",
            Self::InvalidMarker => {
                "Hay marcadores inválidos. Usa solo [n], con un entero positivo sin espacios ni ceros iniciales; varias fuentes se escriben [1][2]. Reserva los corchetes exclusivamente para citas."
            }
            Self::UnknownSource => "Hay citas a identificadores que no existen en el contexto.",
        }
    }
}

/// Validate reference syntax and existence, not whether evidence entails a claim.
pub(super) fn validate(text: String, sources: &[Source]) -> Result<Answer, CitationIssue> {
    if text.trim() == NO_EVIDENCE {
        return Ok(Answer::no_evidence());
    }
    if text.trim().is_empty() {
        return Err(CitationIssue::EmptyResponse);
    }
    let available: HashMap<&str, &Source> = sources
        .iter()
        .map(|source| (source.id.as_str(), source))
        .collect();
    let mut seen = HashSet::new();
    let mut cited = Vec::new();
    let mut chars = text.char_indices();
    while let Some((start, ch)) = chars.next() {
        match ch {
            '[' => {
                let (end, _) = chars
                    .by_ref()
                    .find(|(_, ch)| *ch == ']')
                    .ok_or(CitationIssue::InvalidMarker)?;
                let id = &text[start + 1..end];
                if id.is_empty()
                    || id.starts_with('0')
                    || !id.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(CitationIssue::InvalidMarker);
                }
                let source = available.get(id).ok_or(CitationIssue::UnknownSource)?;
                if seen.insert(id) {
                    cited.push((*source).clone());
                }
            }
            ']' => return Err(CitationIssue::InvalidMarker),
            _ => {}
        }
    }
    if cited.is_empty() {
        return Err(CitationIssue::MissingCitation);
    }
    Ok(Answer {
        text,
        sources: cited,
    })
}
