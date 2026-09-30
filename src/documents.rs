use crate::ingest::SkippedFile;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub(crate) struct Document {
    pub relative_path: String,
    pub text: String,
    pub content_hash: String,
}

pub(crate) struct DocumentInventory {
    pub documents: Vec<Document>,
    pub skipped: Vec<SkippedFile>,
    pub files_seen: usize,
    pub present_paths: BTreeSet<String>,
    pub protected_paths: BTreeSet<String>,
}

impl DocumentInventory {
    pub fn contains(&self, relative_path: &str) -> bool {
        self.present_paths.contains(relative_path)
            || self.protected_paths.iter().any(|path| {
                relative_path
                    .strip_prefix(path)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            })
    }
}

pub(crate) fn read_documents(root: &Path, max_bytes: u64) -> Result<DocumentInventory> {
    if !root.is_dir() {
        bail!(
            "La carpeta fuente no existe o no es directorio: {}",
            root.display()
        );
    }
    let mut paths: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| entry.map(|e| e.path().to_path_buf()))
        .collect::<Result<_, _>>()
        .context("No se pudo recorrer la carpeta fuente")?;
    paths.sort();
    let mut documents = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = 0;
    let mut present_paths = BTreeSet::new();
    let mut protected_paths = BTreeSet::new();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .context("Ruta fuera de la carpeta fuente")?
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .context("Una ruta no es UTF-8")
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        if relative.is_empty() {
            continue;
        }
        present_paths.insert(relative.clone());
        if path.is_symlink() {
            // No recorrer un enlace no significa que sus documentos desaparecieron.
            protected_paths.insert(relative.clone());
            seen += 1;
            skipped.push(SkippedFile {
                path: relative,
                reason: "enlace simbólico".into(),
            });
            continue;
        }
        if !path.is_file() {
            continue;
        }
        seen += 1;
        if !matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("txt" | "md")
        ) {
            skipped.push(SkippedFile {
                path: relative,
                reason: "formato no admitido".into(),
            });
            continue;
        }
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("No se pudo leer {}", path.display()))?;
        if metadata.len() > max_bytes {
            bail!("Archivo demasiado grande: {}", relative);
        }
        let bytes =
            std::fs::read(&path).with_context(|| format!("No se pudo leer {}", relative))?;
        if bytes.len() as u64 > max_bytes {
            bail!("Archivo demasiado grande: {}", relative);
        }
        let text = String::from_utf8(bytes)
            .with_context(|| format!("El archivo no es UTF-8: {}", relative))?;
        if text.trim().is_empty() {
            skipped.push(SkippedFile {
                path: relative,
                reason: "archivo vacío".into(),
            });
            continue;
        }
        let content_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        documents.push(Document {
            relative_path: relative,
            text,
            content_hash,
        });
    }
    Ok(DocumentInventory {
        documents,
        skipped,
        files_seen: seen,
        present_paths,
        protected_paths,
    })
}
