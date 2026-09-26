use crate::documents::Document;
use crate::{Chunk, Config};

pub(crate) fn chunk_document(doc: &Document, config: &Config) -> Vec<Chunk> {
    let mut result = Vec::new();
    let mut section: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    let mut current_section: Option<String> = None;
    let flush = |words: &mut Vec<String>,
                 current_section: &Option<String>,
                 result: &mut Vec<Chunk>,
                 carry: bool| {
        if words.is_empty() {
            return;
        }
        let mut start = 0;
        while start < words.len() {
            let end = (start + config.chunk_target_tokens).min(words.len());
            result.push(Chunk {
                text: words[start..end].join(" "),
                relative_path: doc.relative_path.clone(),
                section: current_section.clone(),
                chunk_index: result.len(),
                content_hash: doc.content_hash.clone(),
                embedding_version: config.embedding_version(),
                embedding_model: config.embedding_model.clone(),
                embedding_dimension: config.embedding_dimension,
                embedding_preprocessing: crate::config::EMBEDDING_PREPROCESSING.to_string(),
            });
            if end == words.len() {
                break;
            }
            start = end.saturating_sub(config.chunk_overlap_tokens);
        }
        if carry && config.chunk_overlap_tokens > 0 {
            let start = words.len().saturating_sub(config.chunk_overlap_tokens);
            words.drain(..start);
        } else {
            words.clear();
        }
    };
    for paragraph in doc.text.split("\n\n") {
        let mut paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if paragraph.starts_with('#') {
            flush(&mut words, &current_section, &mut result, false);
            let (heading, rest) = paragraph.split_once('\n').unwrap_or((paragraph, ""));
            section = Some(heading.trim_start_matches('#').trim().to_string());
            current_section = section.clone();
            paragraph = rest.trim();
            if paragraph.is_empty() {
                continue;
            }
        }
        let paragraph_words: Vec<String> =
            paragraph.split_whitespace().map(str::to_string).collect();
        if !words.is_empty() && words.len() + paragraph_words.len() > config.chunk_max_tokens {
            flush(&mut words, &current_section, &mut result, true);
        }
        if words.is_empty() {
            current_section = section.clone();
        }
        words.extend(paragraph_words);
    }
    flush(&mut words, &current_section, &mut result, false);
    result
}
