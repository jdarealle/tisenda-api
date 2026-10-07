use crate::Config;
use anyhow::{Context, Result, bail};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub docling_url: reqwest::Url,
    pub max_download_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_archive_entries: usize,
    pub request_timeout_secs: u64,
    pub conversion_timeout_secs: u64,
    pub image_export_mode: String,
    pub do_ocr: bool,
    pub ocr_preset: String,
    pub documents_root: PathBuf,
    pub max_original_bytes: u64,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self> {
        fn value(name: &str, default: &str) -> String {
            std::env::var(name).unwrap_or_else(|_| default.to_string())
        }
        Ok(Self {
            listen: value("HTTP_BIND", "0.0.0.0:3000")
                .parse()
                .context("HTTP_BIND inválido")?,
            docling_url: value("DOCLING_URL", "http://127.0.0.1:5001/")
                .parse()
                .context("DOCLING_URL inválida")?,
            max_download_bytes: value("DOCLING_MAX_DOWNLOAD_BYTES", "33554432").parse()?,
            max_expanded_bytes: value("DOCLING_MAX_EXPANDED_BYTES", "134217728").parse()?,
            max_archive_entries: value("DOCLING_MAX_ARCHIVE_ENTRIES", "200").parse()?,
            request_timeout_secs: value("DOCLING_REQUEST_TIMEOUT_SECS", "120").parse()?,
            conversion_timeout_secs: value("DOCLING_CONVERSION_TIMEOUT_SECS", "1800").parse()?,
            image_export_mode: value("DOCLING_IMAGE_EXPORT_MODE", "referenced"),
            do_ocr: value("DOCLING_DO_OCR", "true")
                .parse()
                .context("DOCLING_DO_OCR debe ser true o false")?,
            ocr_preset: value("DOCLING_OCR_PRESET", "auto"),
            documents_root: value("DOCUMENTS_ROOT", "manuales").into(),
            max_original_bytes: value("DOCLING_SERVE_MAX_FILE_SIZE", "52428800").parse()?,
        })
    }
    pub(crate) fn validate(&mut self, config: &Config) -> Result<()> {
        if !matches!(self.docling_url.scheme(), "http" | "https")
            || self.docling_url.host_str().is_none()
            || self.docling_url.query().is_some()
            || self.docling_url.fragment().is_some()
            || !self.docling_url.username().is_empty()
            || self.docling_url.password().is_some()
        {
            bail!("DOCLING_URL debe ser una URL HTTP válida sin credenciales, query ni fragmento");
        }
        if !self.docling_url.path().ends_with('/') {
            self.docling_url
                .set_path(&format!("{}/", self.docling_url.path()));
        }
        if self.max_download_bytes == 0
            || self.max_expanded_bytes == 0
            || self.max_archive_entries == 0
            || self.request_timeout_secs == 0
            || self.max_expanded_bytes == u64::MAX
            || config.max_file_bytes == u64::MAX
            || self.conversion_timeout_secs == 0
            || self.max_original_bytes == 0
        {
            bail!("Límites HTTP/ZIP inválidos");
        }
        if !matches!(
            self.image_export_mode.as_str(),
            "referenced" | "embedded" | "placeholder"
        ) {
            bail!("DOCLING_IMAGE_EXPORT_MODE debe ser referenced, embedded o placeholder");
        }
        if self.do_ocr && self.ocr_preset.trim().is_empty() {
            bail!("DOCLING_OCR_PRESET no puede estar vacío cuando DOCLING_DO_OCR=true");
        }
        Ok(())
    }
}
