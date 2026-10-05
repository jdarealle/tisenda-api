use crate::DoclingChunk;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

pub(super) fn attach(
    chunk: &mut DoclingChunk,
    document: Option<&Value>,
    sha: Option<&str>,
    profile: &Value,
    count: Option<usize>,
) -> Result<()> {
    let mut items = Vec::new();
    if let Some(document) = document {
        for reference in &chunk.doc_items {
            let pointer = reference.strip_prefix('#').unwrap_or(reference);
            let Some(item) = document.pointer(pointer) else {
                bail!("Referencia Docling no resuelta: {reference}");
            };
            let mut ancestors = Vec::new();
            let mut parent = item
                .get("parent")
                .and_then(|p| p.get("$ref"))
                .and_then(Value::as_str);
            for _ in 0..64 {
                let Some(reference) = parent else {
                    break;
                };
                let Some(node) = document.pointer(reference.trim_start_matches('#')) else {
                    break;
                };
                ancestors.push(json!({"self_ref": reference, "name": node.get("name"), "label": node.get("label")}));
                parent = node
                    .get("parent")
                    .and_then(|p| p.get("$ref"))
                    .and_then(Value::as_str);
            }
            items.push(json!({"self_ref": reference, "label": item.get("label"), "prov": item.get("prov"), "ancestors": ancestors}));
        }
    }
    chunk.metadata.get_or_insert_default().insert(
        "rag".into(),
        json!({
            "schema": 4, "original_sha256": sha, "profile": profile,
            "tei_num_tokens": count, "provenance": items,
            "provenance_precision": "docling_item", "source_filename": chunk.filename
        }),
    );
    Ok(())
}

/// Referenced resources must stay inside the exported archive; never fetch external URIs.
pub(super) fn validate_images(
    document: &Value,
    document_name: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    fn walk(value: &Value, base: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
        match value {
            Value::Object(object) => {
                if let Some(uri) = object
                    .get("image")
                    .and_then(|i| i.get("uri"))
                    .and_then(Value::as_str)
                    && !uri.starts_with("data:")
                {
                    let decoded = percent_encoding::percent_decode_str(uri).decode_utf8()?;
                    let path = Path::new(decoded.as_ref());
                    if decoded.contains(['\\', ':'])
                        || path
                            .components()
                            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
                    {
                        bail!("Referencia de imagen fuera del ZIP: {uri}");
                    }
                    let name = base.join(decoded.trim_start_matches("./"));
                    if !name.to_str().is_some_and(|n| files.contains_key(n)) {
                        bail!("Falta el recurso de imagen {uri}");
                    }
                }
                for child in object.values() {
                    walk(child, base, files)?;
                }
            }
            Value::Array(array) => {
                for child in array {
                    walk(child, base, files)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(
        document,
        Path::new(document_name).parent().unwrap_or(Path::new("")),
        files,
    )
}
