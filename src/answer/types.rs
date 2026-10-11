use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};
use utoipa::ToSchema;

pub(super) const NO_EVIDENCE: &str =
    "No hay información suficiente en los documentos indexados para responder esa pregunta.";

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerRequest {
    /// Pregunta obligatoria con al menos un carácter que no sea espacio.
    #[schema(
        min_length = 1,
        examples("¿Qué consumible utiliza la impresora de recepción?")
    )]
    pub question: String,
}

/// Fragmento citado con marcador local e identidad estable del documento.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Source {
    /// Cadena numérica que vincula esta fuente con un marcador [n] del texto.
    pub id: String,
    /// Identidad estable del documento original en el catálogo.
    pub document_id: uuid::Uuid,
    /// Nombre visible del archivo original.
    pub filename: String,
    /// Ruta relativa del original dentro de la raíz documental.
    pub source_key: String,
    pub location: SourceLocation,
    /// Texto indexado exacto enviado al modelo, incluido su contexto estructural.
    pub excerpt: String,
}

/// Ubicación disponible; la categoría del formato no garantiza coordenadas precisas.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SourceLocation {
    /// Categoría: page, slide, sheet, section, image o document.
    pub kind: String,
    /// Páginas conocidas del PDF; se omite si no están disponibles.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub page_numbers: Option<Vec<usize>>,
    /// Encabezados conocidos; se omite si no están disponibles.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub headings: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[schema(examples(json!({
    "text": "Apaga la impresora antes de sustituir el cartucho. [1]",
    "sources": [{
        "id": "1",
        "document_id": "019934a0-22c0-7000-8000-000000000001",
        "filename": "manual-impresora.pdf",
        "source_key": "impresoras/manual-impresora.pdf",
        "location": { "kind": "page", "page_numbers": [12], "headings": ["Mantenimiento"] },
        "excerpt": "Apague la impresora antes de sustituir el cartucho."
    }]
})))]
pub struct Answer {
    /// Respuesta con marcadores [n], o mensaje de ausencia de evidencia.
    pub text: String,
    /// Fuentes citadas, sin duplicados y en orden de primera aparición; resolver por id.
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
