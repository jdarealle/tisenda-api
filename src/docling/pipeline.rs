use super::{ArchiveLimits, Client, FileReport, provenance};
use crate::{Config, DoclingChunk, IngestDocument, ingest::ingest_with_progress, tei::TeiModel};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;

/// Convert, validate and index one original document.
pub(crate) async fn process_original(
    config: &Config,
    client: &Client,
    limits: ArchiveLimits,
    directory: &Path,
    report: &mut FileReport,
    progress: &crate::ingestions::Progress,
) -> Result<()> {
    progress.stage(report, "conversion").await?;
    let task = client
        .submit(
            config,
            &directory.join("original"),
            &report.filename,
            config.chunk_target_tokens,
            false,
        )
        .await?;
    report.task_ids.push(task.clone());
    progress.save(report).await?;
    let status = client.wait(&task).await?;
    match status["task_status"].as_str() {
        Some("success") => {}
        Some("partial_success") => report
            .warnings
            .push(format!("Conversión parcial: {status}")),
        _ => bail!("Conversión no satisfactoria: {status}"),
    }
    progress.stage(report, "download").await?;
    let artifact = client.result(&task, limits.clone()).await?;
    let (document_name, document) = artifact.document()?;
    provenance::validate_images(&document, &document_name, &artifact.files)?;
    tokio::fs::write(
        directory.join("document.json"),
        serde_json::to_vec(&document)?,
    )
    .await?;
    let chunks = artifact.chunks(&report.filename, false)?;
    drop(artifact);
    finish(
        config,
        client,
        limits,
        directory,
        report,
        progress,
        Converted { chunks, document },
    )
    .await
}

struct Converted {
    chunks: Vec<DoclingChunk>,
    document: Value,
}

async fn finish(
    config: &Config,
    client: &Client,
    limits: ArchiveLimits,
    directory: &Path,
    report: &mut FileReport,
    progress: &crate::ingestions::Progress,
    converted: Converted,
) -> Result<()> {
    let Converted {
        mut chunks,
        document,
    } = converted;
    progress.stage(report, "tokens").await?;
    let tei = TeiModel::new(config)?;
    let identity = tei.check_model().await?;
    let limit = tei.input_limit().await?;
    let mut budget = config.chunk_target_tokens;
    let mut previous = serde_json::to_vec(&chunks)?;
    let mut attempt = 0;
    let counts = loop {
        let counts = tei.chunk_counts(&chunks, limit).await?;
        if counts
            .iter()
            .all(|n| n.is_some_and(|n| n > 0 && n <= limit))
        {
            break counts;
        }
        if attempt == 3 || budget <= 1 {
            bail!(
                "Docling no produjo fragmentos compatibles con TEI ({limit} tokens) tras {attempt} recuperaciones"
            );
        }
        attempt += 1;
        budget = (budget / 2).max(1);
        tracing::debug!(event = "document_rechunk", attempt, budget);
        progress.stage(report, "rechunk").await?;
        let task = client
            .submit(
                config,
                &directory.join("document.json"),
                "document.json",
                budget,
                true,
            )
            .await?;
        report.task_ids.push(task.clone());
        progress.save(report).await?;
        let status = client.wait(&task).await?;
        if status["task_status"] != "success" {
            bail!("Falló la recuperación Docling: {status}");
        }
        let artifact = client.result(&task, limits.clone()).await?;
        chunks = artifact.chunks(&report.filename, true)?;
        let serialized = serde_json::to_vec(&chunks)?;
        if serialized == previous {
            bail!("Docling repitió los mismos fragmentos incompatibles; recuperación detenida");
        }
        previous = serialized;
        progress.stage(report, "tokens").await?;
    };
    if tei.check_model().await? != identity {
        bail!("TEI cambió de revisión durante la validación");
    }
    let profile = json!({"conversion": report.profile, "effective_budget": budget,
        "recoveries": attempt, "embedding_identity": identity, "max_input_length": limit});
    for (chunk, count) in chunks.iter_mut().zip(counts) {
        provenance::attach(
            chunk,
            Some(&document),
            report.original_sha256.as_deref(),
            &profile,
            count,
        )?;
    }
    report.chunks = chunks.len();
    progress.stage(report, "index").await?;
    let results = ingest_with_progress(
        config,
        vec![IngestDocument {
            document_id: report.document_id,
            filename: report.filename.clone(),
            source_key: report.source_key.clone(),
            chunks,
        }],
        Some(progress),
    )
    .await;
    let indexed = results
        .into_iter()
        .next()
        .context("Falta resultado de indexación")?
        .result?;
    report.result = Some(
        if indexed.files_unchanged == 1 {
            "unchanged"
        } else {
            "indexed"
        }
        .into(),
    );
    report.status = if report.warnings.is_empty() {
        "completed"
    } else {
        "completed_with_warnings"
    }
    .into();
    report.stage = "done".into();
    Ok(())
}
