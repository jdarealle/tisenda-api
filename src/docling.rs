use crate::{
    Config, DoclingChunk, IngestDocument,
    ingest::{validate_chunks, validate_filename},
    ingest_documents,
};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, Cursor, Read},
    path::Path,
};

#[derive(Debug, Deserialize)]
pub(crate) struct Callback {
    pub(crate) task_id: String,
    pub(crate) progress: Progress,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Progress {
    SetNumDocs,
    DocumentCompleted,
    UpdateProcessed(Batch),
}

#[derive(Debug, Deserialize)]
struct ProcessedDocument {
    source: String,
    status: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Batch {
    num_processed: usize,
    num_succeeded: usize,
    num_partially_succeeded: usize,
    num_failed: usize,
    docs: Vec<ProcessedDocument>,
}

impl Batch {
    pub(crate) fn validate(&self) -> Result<()> {
        let succeeded = self.docs.iter().filter(|d| d.status == "success").count();
        let partial = self
            .docs
            .iter()
            .filter(|d| d.status == "partial_success")
            .count();
        if self.docs.is_empty()
            || self.num_processed != self.docs.len()
            || self.num_succeeded != succeeded
            || self.num_partially_succeeded != partial
            || self.num_failed != self.docs.len() - succeeded - partial
        {
            bail!("Resumen de conversión inconsistente");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ArchiveLimits {
    pub(crate) download: u64,
    pub(crate) expanded: u64,
    pub(crate) file: u64,
    pub(crate) entries: usize,
}

async fn download(
    client: &reqwest::Client,
    base: &reqwest::Url,
    task_id: &str,
    maximum: u64,
) -> Result<Vec<u8>> {
    // UUID validation prevents a callback from selecting an arbitrary URL or path.
    uuid::Uuid::parse_str(task_id)?;
    let url = base.join(&format!("v1/result/{task_id}"))?;
    let response = client.get(url).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|size| size > maximum) {
        bail!("Resultado demasiado grande");
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next())
        != Some("application/zip")
    {
        bail!("Docling debe devolver target_type=zip");
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > maximum {
            bail!("Resultado demasiado grande");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Read the native flat chunks JSONL ZIP, checking declared and actual expanded sizes.
/// No entry is ever extracted to the filesystem.
fn archive_files(bytes: Vec<u8>, limits: &ArchiveLimits) -> Result<HashMap<String, Vec<u8>>> {
    if bytes.len() as u64 > limits.download {
        bail!("ZIP demasiado grande");
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("ZIP inválido")?;
    if zip.len() > limits.entries {
        bail!("Demasiadas entradas en el ZIP");
    }
    let mut expanded = 0u64;
    let mut declared = 0u64;
    let mut files = HashMap::new();
    let mut names = HashSet::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_string();
        if entry.enclosed_name().is_none() || name.contains('\\') || !names.insert(name.clone()) {
            bail!("Nombre de entrada inválido o duplicado");
        }
        declared = declared
            .checked_add(entry.size())
            .context("ZIP demasiado grande")?;
        if declared > limits.expanded {
            bail!("ZIP expandido demasiado grande");
        }
        if entry.is_dir() {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            bail!("El ZIP contiene un enlace simbólico");
        }
        if !name.ends_with(".chunks.jsonl") {
            continue;
        }
        validate_filename(&name)?;
        if entry.size() > limits.file {
            bail!("Archivo convertido demasiado grande");
        }
        let maximum = limits.file.min(limits.expanded - expanded);
        let mut data = Vec::new();
        (&mut entry).take(maximum + 1).read_to_end(&mut data)?;
        if data.len() as u64 > maximum {
            bail!("Archivo expandido demasiado grande");
        }
        expanded += data.len() as u64;
        files.insert(name, data);
    }
    Ok(files)
}

struct ConvertedDocument {
    filename: String,
    document: Result<IngestDocument>,
}

fn converted_documents(
    bytes: Vec<u8>,
    batch: Batch,
    limits: &ArchiveLimits,
    max_tokens: usize,
) -> Result<Vec<ConvertedDocument>> {
    let files = archive_files(bytes, limits)?;
    let mut stems = HashMap::new();
    for doc in &batch.docs {
        if let Some(stem) = Path::new(&doc.source).file_stem().and_then(|s| s.to_str()) {
            *stems.entry(stem.to_string()).or_insert(0usize) += 1;
        }
    }
    Ok(batch
        .docs
        .into_iter()
        .map(|doc| {
            let filename = doc.source;
            let document = (|| {
                validate_filename(&filename)?;
                if doc.status != "success" {
                    bail!("Conversión no satisfactoria");
                }
                let stem = Path::new(&filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("Nombre inválido")?;
                if stems.get(stem) != Some(&1) {
                    bail!("Nombre base ambiguo");
                }
                let bytes = files
                    .get(&format!("{stem}.chunks.jsonl"))
                    .context("Faltan fragmentos JSONL de Docling")?;
                let mut chunks = Vec::new();
                for (line_index, line) in Cursor::new(bytes).split(b'\n').enumerate() {
                    let line = line?;
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let chunk: DoclingChunk = serde_json::from_slice(&line).with_context(|| {
                        format!("Registro JSONL inválido en la línea {}", line_index + 1)
                    })?;
                    chunks.push(chunk);
                }
                validate_chunks(&filename, &chunks, max_tokens)?;
                Ok(IngestDocument {
                    filename: filename.clone(),
                    chunks,
                })
            })();
            ConvertedDocument { filename, document }
        })
        .collect())
}

pub(crate) fn log_result(task_id: &str, filename: Option<&str>, success: bool) {
    if success {
        tracing::info!(task_id, document = filename, "ingesta_satisfactoria");
    } else {
        tracing::error!(task_id, document = filename, "ingesta_fallida");
    }
}

pub(crate) async fn process(
    config: &Config,
    client: &reqwest::Client,
    base: &reqwest::Url,
    limits: ArchiveLimits,
    task_id: &str,
    batch: Batch,
) {
    if batch.docs.iter().all(|doc| doc.status != "success") {
        for doc in batch.docs {
            log_result(task_id, Some(&doc.source), false);
        }
        return;
    }
    let max_tokens = config.chunk_max_tokens;
    let result = async {
        let bytes = download(client, base, task_id, limits.download).await?;
        tokio::task::spawn_blocking(move || converted_documents(bytes, batch, &limits, max_tokens))
            .await?
    }
    .await;
    let documents = match result {
        Ok(documents) => documents,
        Err(_) => {
            log_result(task_id, None, false);
            return;
        }
    };
    for converted in documents {
        match converted.document {
            Ok(document) => {
                let results = ingest_documents(config, vec![document]).await;
                for result in results {
                    log_result(task_id, Some(&result.filename), result.result.is_ok());
                }
            }
            Err(_) => log_result(task_id, Some(&converted.filename), false),
        }
    }
}
