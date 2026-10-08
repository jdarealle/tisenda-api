use super::{ArchiveLimits, archive::Artifact};
use crate::{Config, ServerConfig};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    server: ServerConfig,
}

impl Client {
    pub(crate) fn new(server: &ServerConfig) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(server.request_timeout_secs))
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
            server: server.clone(),
        })
    }

    fn force_ocr(&self, filename: &str) -> bool {
        self.server.do_ocr && super::input_format(filename) == Some("image")
    }

    pub(crate) fn profile(&self, config: &Config, filename: &str) -> Value {
        json!({
            "version": 5, "docling_serve": "1.36.0",
            "chunker": "hybrid", "tokenizer": config.embedding_model,
            "embedding_revision": config.embedding_revision,
            "max_tokens": config.chunk_target_tokens, "merge_peers": true,
            "include_raw_text": true, "use_markdown_tables": false, "use_markdown_images": false,
            "image_export_mode": self.server.image_export_mode,
            "do_ocr": self.server.do_ocr,
            "ocr_preset": if self.server.do_ocr { Some(&self.server.ocr_preset) } else { None },
            "force_ocr": self.force_ocr(filename), "do_table_structure": true, "table_mode": "accurate",
            "do_pdf_heading_hierarchy": true, "do_picture_description": false,
            "include_images": self.server.image_export_mode != "placeholder", "include_page_images": false,
            "recovery": "docling-json-half-budget-max-3-v1", "token_validation": "tei-special-tokens-v1"
        })
    }

    pub(super) async fn submit(
        &self,
        config: &Config,
        path: &Path,
        filename: &str,
        budget: usize,
        recovery: bool,
    ) -> Result<String> {
        crate::logging::operation("docling", "submit", async {
            let file = tokio::fs::File::open(path).await?;
            let size = file.metadata().await?.len();
            let part = Part::stream_with_length(file, size).file_name(filename.to_owned());
            let mut form = Form::new().part("files", part).text("target_type", "zip");
            let endpoint;
            if recovery {
                endpoint = "v1/chunk/hybrid/file/async";
                // This endpoint uses flattened, prefixed fields (not chunking_options JSON).
                form = form
                    .text("chunking_tokenizer", config.embedding_model.clone())
                    .text("chunking_max_tokens", budget.to_string())
                    .text("chunking_merge_peers", "true")
                    .text("chunking_include_raw_text", "true")
                    .text("chunking_use_markdown_tables", "false")
                    .text("chunking_use_markdown_images", "false")
                    .text("convert_from_formats", "json_docling")
                    .text("convert_do_ocr", "false")
                    .text("convert_do_picture_description", "false")
                    .text("convert_include_images", "false")
                    .text("convert_image_export_mode", "placeholder")
                    .text("include_converted_doc", "false");
            } else {
                endpoint = "v1/convert/file/async";
                let format = super::input_format(filename).context("Extensión no admitida")?;
                form = form
                    .text("from_formats", format)
                    .text("to_formats", "json")
                    .text("to_formats", "chunks")
                    .text(
                        "chunking_options",
                        json!({
                            "chunker": "hybrid", "tokenizer": config.embedding_model,
                            "max_tokens": budget, "merge_peers": true, "include_raw_text": true,
                            "use_markdown_tables": false, "use_markdown_images": false
                        })
                        .to_string(),
                    )
                    .text("image_export_mode", self.server.image_export_mode.clone())
                    .text(
                        "include_images",
                        (self.server.image_export_mode != "placeholder").to_string(),
                    )
                    .text("include_page_images", "false")
                    .text("do_ocr", self.server.do_ocr.to_string())
                    .text("force_ocr", self.force_ocr(filename).to_string())
                    .text("do_table_structure", "true")
                    .text("table_mode", "accurate")
                    .text("do_pdf_heading_hierarchy", "true")
                    .text("do_picture_description", "false")
                    .text("do_picture_classification", "false")
                    .text("do_chart_extraction", "false")
                    .text("abort_on_error", "false");
                if self.server.do_ocr {
                    form = form.text("ocr_preset", self.server.ocr_preset.clone());
                }
            }
            let response = self
                .http
                .post(self.server.docling_url.join(endpoint)?)
                .multipart(form)
                .send()
                .await?;
            let value = response_json(response).await?;
            let task = value["task_id"]
                .as_str()
                .context("Docling no devolvió task_id")?;
            uuid::Uuid::parse_str(task).context("task_id inválido")?;
            Ok(task.to_owned())
        })
        .await
    }

    pub(super) async fn wait(&self, task: &str) -> Result<Value> {
        crate::logging::operation("docling", "wait", async {
            uuid::Uuid::parse_str(task)?;
            let started = Instant::now();
            loop {
                let response = self
                    .http
                    .get(
                        self.server
                            .docling_url
                            .join(&format!("v1/status/poll/{task}"))?,
                    )
                    .send()
                    .await?;
                let value = response_json(response).await?;
                match value["task_status"].as_str() {
                    Some("success" | "partial_success" | "failure" | "skipped") => {
                        return Ok(value);
                    }
                    Some(state @ ("pending" | "started")) => {
                        tracing::trace!(event = "docling_poll", state);
                    }
                    status => bail!("Estado desconocido de Docling: {status:?}"),
                }
                if started.elapsed().as_secs() >= self.server.conversion_timeout_secs {
                    return Err(crate::ingestions::ConversionTimeout(task.into()).into());
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
        .await
    }

    pub(super) async fn result(&self, task: &str, limits: ArchiveLimits) -> Result<Artifact> {
        crate::logging::operation("docling", "result", async {
            uuid::Uuid::parse_str(task)?;
            let response = self
                .http
                .get(self.server.docling_url.join(&format!("v1/result/{task}"))?)
                .send()
                .await?
                .error_for_status()?;
            if response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.split(';').next())
                != Some("application/zip")
            {
                bail!("Docling debe devolver un ZIP");
            }
            if response
                .content_length()
                .is_some_and(|n| n > limits.download)
            {
                bail!("ZIP demasiado grande");
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if (bytes.len() as u64).saturating_add(chunk.len() as u64) > limits.download {
                    bail!("ZIP demasiado grande");
                }
                bytes.extend_from_slice(&chunk);
            }
            let span = tracing::Span::current();
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            let artifact = tokio::task::spawn_blocking(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    span.in_scope(|| Artifact::read(bytes, &limits))
                })
            })
            .await??;
            Ok(artifact)
        })
        .await
    }
}

async fn response_json(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
            bail!("Respuesta de control Docling demasiado grande");
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        return Err(crate::logging::HttpFailure::new(
            status.as_u16(),
            format!("Docling HTTP {status}: {}", String::from_utf8_lossy(&bytes)),
        )
        .into());
    }
    serde_json::from_slice(&bytes).context("Respuesta Docling inválida")
}
