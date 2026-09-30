use anyhow::Context;
use clap::{Parser, Subcommand};
use rag::{AnswerRequest, Config, IngestOptions};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rag", about = "RAG local para manuales de soporte")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sincroniza documentos nuevos o modificados con el índice activo.
    Ingest {
        #[arg(long)]
        source: Option<PathBuf>,
        /// Elimina del índice los documentos ausentes de la carpeta fuente.
        #[arg(long)]
        prune: bool,
    },
    /// Responde una pregunta usando el índice activo.
    Ask {
        question: String,
        #[arg(long)]
        top_k: Option<usize>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let config = Config::from_env()?;
    match cli.command {
        Command::Ingest { source, prune } => {
            let source = source
                .or_else(|| config.source_dir.clone())
                .context("Define SOURCE_DIR en .env o usa ingest --source <ruta>")?;
            let report = rag::ingest(
                &config,
                IngestOptions {
                    source_dir: source,
                    prune,
                },
            )
            .await?;
            println!(
                "Archivos vistos: {}; admitidos: {}; omitidos: {}; fragmentos en índice: {}",
                report.files_seen, report.files_indexed, report.files_skipped, report.chunks
            );
            println!(
                "Nuevos: {}; actualizados: {}; sin cambios: {}; eliminados: {}; fragmentos escritos: {}",
                report.files_new,
                report.files_updated,
                report.files_unchanged,
                report.files_removed,
                report.chunks_written
            );
            for skipped in &report.skipped {
                println!("Omitido: {} ({})", skipped.path, skipped.reason);
            }
            for path in &report.preserved {
                println!(
                    "Conservado en el índice: {:?} (presente, pero no procesable)",
                    path
                );
            }
            for path in &report.pending_removal {
                println!(
                    "Ausente, conservado en el índice: {:?} (usa ingest --prune para eliminarlo)",
                    path
                );
            }
            println!("Índice activo: {}", report.active_collection);
        }
        Command::Ask { question, top_k } => {
            let answer = rag::answer(&config, AnswerRequest { question, top_k }).await?;
            println!("{}", answer.text);
            if !answer.sources.is_empty() {
                println!("\nFuentes:");
                for (i, source) in answer.sources.iter().enumerate() {
                    println!(
                        "[{}] {}{} (fragmento {}, similitud {:.3})",
                        i + 1,
                        source.relative_path,
                        source
                            .section
                            .as_ref()
                            .map(|s| format!(" — {s}"))
                            .unwrap_or_default(),
                        source.chunk_index,
                        source.score
                    );
                }
            }
        }
    }
    Ok(())
}
