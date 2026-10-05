use super::ArchiveLimits;
use crate::{DoclingChunk, ingest::validate_chunks};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    io::{Cursor, Read},
    path::{Component, Path},
};

pub(super) struct Artifact {
    pub(super) files: BTreeMap<String, Vec<u8>>,
}

impl Artifact {
    pub(super) fn read(bytes: Vec<u8>, limits: &ArchiveLimits) -> Result<Self> {
        if bytes.len() as u64 > limits.download {
            bail!("ZIP demasiado grande");
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("ZIP inválido")?;
        if zip.len() > limits.entries {
            bail!("Demasiadas entradas en el ZIP");
        }
        let mut files = BTreeMap::new();
        let mut names = HashSet::new();
        let mut expanded = 0u64;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            let name = entry.name().to_owned();
            if entry.enclosed_name().is_none()
                || name.contains(['\\', ':'])
                || Path::new(&name)
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || !names.insert(name.clone())
                || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            {
                bail!("Entrada ZIP inválida, duplicada o enlace simbólico");
            }
            if entry.is_dir() {
                continue;
            }
            let maximum = (limits.expanded - expanded).min(if name.ends_with(".chunks.jsonl") {
                limits.file
            } else {
                limits.expanded
            });
            if entry.size() > maximum {
                bail!("Recurso convertido demasiado grande: {name}");
            }
            let mut data = Vec::new();
            (&mut entry).take(maximum + 1).read_to_end(&mut data)?;
            if data.len() as u64 > maximum {
                bail!("ZIP expandido demasiado grande");
            }
            expanded += data.len() as u64;
            files.insert(name, data);
        }
        // Reject a file used as the parent directory of another resource.
        for name in files.keys() {
            for ancestor in Path::new(name).ancestors().skip(1) {
                if ancestor.to_str().is_some_and(|p| files.contains_key(p)) {
                    bail!("Conflicto entre archivo y directorio en el ZIP");
                }
            }
        }
        Ok(Self { files })
    }

    pub(super) fn chunks(&self, filename: &str, recovery: bool) -> Result<Vec<DoclingChunk>> {
        let stem = Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Nombre inválido")?;
        let expected = format!("{stem}.chunks.jsonl");
        let candidates: Vec<_> = self
            .files
            .iter()
            .filter(|(name, _)| {
                name.ends_with(".chunks.jsonl")
                    && (recovery
                        || Path::new(name).file_name().and_then(|s| s.to_str()) == Some(&expected))
            })
            .collect();
        if candidates.len() != 1 {
            bail!("Se esperaba un único JSONL de fragmentos para {filename}");
        }
        let mut chunks: Vec<DoclingChunk> = candidates[0]
            .1
            .split(|b| *b == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .map(serde_json::from_slice)
            .collect::<std::result::Result<_, _>>()?;
        if recovery {
            // The transport filename is document.json; source identity remains the original.
            for chunk in &mut chunks {
                chunk.filename = filename.to_owned();
                if let Some(origin) = chunk.metadata.as_mut().and_then(|m| m.get_mut("origin"))
                    && let Some(object) = origin.as_object_mut()
                {
                    object.insert("filename".into(), filename.into());
                }
            }
        }
        validate_chunks(filename, &chunks)?;
        Ok(chunks)
    }

    pub(super) fn document(&self) -> Result<(String, Value)> {
        let mut result = None;
        for (name, bytes) in &self.files {
            if !name.ends_with(".json") {
                continue;
            }
            let value: Value = serde_json::from_slice(bytes).context("JSON exportado inválido")?;
            if value.get("schema_name").and_then(Value::as_str) == Some("DoclingDocument") {
                if result.is_some() {
                    bail!("Más de un documento estructurado en la tarea");
                }
                result = Some((name.clone(), value));
            }
        }
        result.context(
            "Falta la exportación JSON de DoclingDocument; solicita to_formats=json y chunks",
        )
    }
}
