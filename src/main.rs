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
    /// Reconstruye el índice desde una carpeta local.
    Ingest {
        #[arg(long)]
        source: Option<PathBuf>,
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
        Command::Ingest { source } => {
            let source = source
                .or_else(|| config.source_dir.clone())
                .context("Define SOURCE_DIR en .env o usa ingest --source <ruta>")?;
            let report = rag::ingest(&config, IngestOptions { source_dir: source }).await?;
            println!(
                "Archivos vistos: {}; admitidos: {}; omitidos: {}; fragmentos: {}",
                report.files_seen, report.files_indexed, report.files_skipped, report.chunks
            );
            for skipped in &report.skipped {
                println!("Omitido: {} ({})", skipped.path, skipped.reason);
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
