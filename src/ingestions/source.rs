use anyhow::{Context, Result, bail};
use rustix::fs::{Mode, OFlags, openat};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Component, Path, PathBuf},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
pub(super) struct InvalidSource(anyhow::Error);
impl std::fmt::Display for InvalidSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for InvalidSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn invalid(message: impl Into<String>) -> anyhow::Error {
    InvalidSource(anyhow::anyhow!(message.into())).into()
}

pub(super) struct SourceRoot {
    path: PathBuf,
    directory: File,
}

impl SourceRoot {
    pub(super) fn new(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("DOCUMENTS_ROOT no existe")?;
        let directory = File::open(&path).context("No se pudo abrir DOCUMENTS_ROOT")?;
        if !directory.metadata()?.is_dir() {
            bail!("DOCUMENTS_ROOT debe ser un directorio");
        }
        Ok(Self { path, directory })
    }

    /// Resolve selections before conversion. Hidden entries are never sources.
    pub(super) fn select(&self, paths: &[String]) -> Result<Vec<String>> {
        let mut selected = BTreeSet::new();
        let selections = if paths.is_empty() {
            vec![".".to_owned()]
        } else {
            paths.to_vec()
        };
        for selection in selections {
            let relative = Path::new(&selection);
            if selection.is_empty() || selection.contains(['\\', ':']) || relative.is_absolute() {
                bail!("Ruta relativa inválida: {selection}");
            }
            let mut current = self.path.clone();
            for part in relative.components() {
                match part {
                    Component::CurDir => {}
                    Component::Normal(name) if !name.to_string_lossy().starts_with('.') => {
                        current.push(name);
                        if std::fs::symlink_metadata(&current)?
                            .file_type()
                            .is_symlink()
                        {
                            bail!("No se admiten enlaces simbólicos: {selection}");
                        }
                    }
                    _ => bail!("Ruta fuera de la raíz o entrada oculta: {selection}"),
                }
            }
            let mut pending = vec![current];
            while let Some(path) = pending.pop() {
                let metadata = std::fs::symlink_metadata(&path)
                    .with_context(|| format!("No se pudo seleccionar {}", path.display()))?;
                if metadata.file_type().is_symlink() {
                    bail!("No se admiten enlaces simbólicos: {}", path.display());
                }
                if metadata.is_dir() {
                    for entry in std::fs::read_dir(&path)? {
                        let entry = entry?;
                        if !entry.file_name().to_string_lossy().starts_with('.') {
                            pending.push(entry.path());
                        }
                    }
                } else if metadata.is_file() {
                    let key = path
                        .strip_prefix(&self.path)?
                        .to_str()
                        .context("La ruta documental debe ser UTF-8")?
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    crate::ingest::validate_source_key(&key)?;
                    selected.insert(key);
                } else {
                    bail!(
                        "La selección contiene una entrada que no es archivo regular: {}",
                        path.display()
                    );
                }
            }
        }
        Ok(selected.into_iter().collect())
    }

    /// Open each component relative to the root descriptor without following
    /// symlinks, including replacements made after selection (TOCTOU).
    fn open(&self, key: &str) -> Result<File> {
        crate::ingest::validate_source_key(key)?;
        let mut directory = self.directory.try_clone()?;
        let mut parts = key.split('/').peekable();
        while let Some(part) = parts.next() {
            let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
            if parts.peek().is_some() {
                flags |= OFlags::DIRECTORY;
            }
            directory = File::from(openat(&directory, part, flags, Mode::empty()).with_context(
                || format!("No se pudo abrir el origen {key} sin enlaces simbólicos"),
            )?);
        }
        if !directory.metadata()?.is_file() {
            bail!("El origen no es un archivo regular: {key}");
        }
        Ok(directory)
    }

    pub(super) async fn snapshot(
        &self,
        key: &str,
        destination: &Path,
        limit: u64,
    ) -> Result<String> {
        let file = self.open(key).map_err(InvalidSource)?;
        let size = file.metadata()?.len();
        if size == 0 || size > limit {
            return Err(invalid(format!(
                "Tamaño original inválido: {size} bytes; máximo {limit}"
            )));
        }
        let mut input = tokio::fs::File::from_std(file);
        let mut output = tokio::fs::File::create(destination).await?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut total = 0u64;
        loop {
            let count = input.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            total = total.saturating_add(count as u64);
            if total > limit {
                return Err(invalid(format!(
                    "El original creció más allá de {limit} bytes"
                )));
            }
            output.write_all(&buffer[..count]).await?;
            hash.update(&buffer[..count]);
        }
        if total == 0 {
            return Err(invalid("El original quedó vacío durante la copia"));
        }
        output.flush().await?;
        Ok(format!("{:x}", hash.finalize()))
    }
}
