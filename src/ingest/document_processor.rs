use super::IngestDocument;
use crate::{Config, DoclingChunk, documents::Document};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub(super) fn prepare(config: &Config, doc: IngestDocument) -> Result<Document> {
    let filename = doc.filename;
    validate_chunks(&filename, &doc.chunks, config.chunk_max_tokens)?;
    let mut canonical = serde_json::to_value(&doc.chunks)?;
    canonicalize_json(&mut canonical);
    let bytes = serde_json::to_vec(&canonical)?;
    if bytes.len() as u64 > config.max_file_bytes {
        bail!("Documento JSONL demasiado grande");
    }
    let content_hash = format!("{:x}", Sha256::digest(&bytes));
    Ok(Document {
        filename,
        chunks: doc.chunks,
        content_hash,
    })
}

pub(crate) fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        bail!("Nombre de documento inválido");
    }
    Ok(())
}

/// Validate the complete document before any external write.
pub(crate) fn validate_chunks(
    filename: &str,
    chunks: &[DoclingChunk],
    max_tokens: usize,
) -> Result<()> {
    validate_filename(filename)?;
    if chunks.is_empty() {
        bail!("El documento no contiene fragmentos");
    }
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.filename != filename || chunk.chunk_index != index {
            bail!("Nombre o secuencia de fragmentos no coincide con el documento");
        }
        if chunk.text.trim().is_empty() {
            bail!("El fragmento no contiene texto");
        }
        let tokens = chunk
            .num_tokens
            .context("HybridChunker debe informar num_tokens")?;
        if tokens == 0 || tokens > max_tokens {
            bail!("El fragmento excede el límite de tokens admitido");
        }
        if chunk
            .page_numbers
            .as_ref()
            .is_some_and(|pages| pages.contains(&0))
        {
            bail!("Las páginas deben comenzar en uno");
        }
        if let Some(origin) = chunk
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("origin"))
            && origin.get("filename").and_then(|name| name.as_str()) != Some(filename)
        {
            bail!("El origen del fragmento no coincide con el documento");
        }
    }
    Ok(())
}

// JSON objects have no meaningful key order; normalize nested metadata before hashing.
fn canonicalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            let ordered: std::collections::BTreeMap<_, _> =
                std::mem::take(fields).into_iter().collect();
            for (key, mut child) in ordered {
                canonicalize_json(&mut child);
                fields.insert(key, child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(canonicalize_json),
        _ => {}
    }
}
