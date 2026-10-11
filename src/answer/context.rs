use super::{Source, SourceLocation};
use crate::documents::Chunk;
use rig::vector_store::VectorSearchResult;
use std::{collections::HashMap, path::Path};

const MAX_CONTEXT_BYTES: usize = 12_000;

/// Sources and prompt context are built together so their identifiers cannot drift.
pub(super) struct PreparedContext {
    text: String,
    sources: Vec<Source>,
}

impl PreparedContext {
    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn sources(&self) -> &[Source] {
        &self.sources
    }
}

pub(super) fn prepare(results: Vec<VectorSearchResult<Chunk>>, top_k: usize) -> PreparedContext {
    let mut sources = Vec::new();
    let mut context = String::new();
    let mut per_document: HashMap<uuid::Uuid, usize> = HashMap::new();
    for VectorSearchResult {
        score,
        document: chunk,
        ..
    } in results
    {
        if score < 0.25 || sources.len() >= top_k {
            break;
        }
        let source_key = chunk.source_key;
        let document_id = chunk.document_id;
        let chunk = chunk.native;
        let count = per_document.entry(document_id).or_default();
        if *count >= 3 {
            continue;
        }
        let kind = location_kind(&chunk.filename);
        let headings = chunk.headings.filter(|values| !values.is_empty());
        let page_numbers = if kind == "page" {
            chunk.page_numbers.filter(|values| !values.is_empty())
        } else {
            None
        };
        let pages = page_numbers
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let id = (sources.len() + 1).to_string();
        let line = format!(
            "[{id}] {} | encabezados: {} | páginas: {pages} | fragmento {}\n{}\n\n",
            chunk.filename,
            headings.as_deref().unwrap_or_default().join(" > "),
            chunk.chunk_index,
            chunk.text
        );
        if context.len() + line.len() > MAX_CONTEXT_BYTES {
            continue;
        }
        context.push_str(&line);
        *count += 1;
        sources.push(Source {
            id,
            document_id,
            filename: chunk.filename,
            source_key,
            location: SourceLocation {
                kind: kind.into(),
                page_numbers,
                headings,
            },
            excerpt: chunk.text,
        });
    }
    PreparedContext {
        text: context,
        sources,
    }
}

fn location_kind(filename: &str) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "page",
        "ppt" | "pptx" | "potx" | "ppsx" | "pptm" | "potm" | "ppsm" | "odp" => "slide",
        "xls" | "xlsx" | "xlsm" | "xltx" | "xltm" | "ods" | "csv" => "sheet",
        "doc" | "docx" | "dotx" | "docm" | "dotm" | "odt" | "md" | "txt" | "text" | "qmd"
        | "rmd" | "adoc" | "asciidoc" | "asc" | "html" | "htm" | "xhtml" => "section",
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "webp" => "image",
        _ => "document",
    }
}
