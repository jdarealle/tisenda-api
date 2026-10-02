use clap::{Parser, Subcommand};
use rag::{Config, ServerConfig};

#[derive(Parser)]
#[command(name = "rag", about = "RAG local para manuales de soporte")]
struct Cli {
    /// Lee únicamente variables del proceso, sin cargar .env.
    #[arg(long, global = true)]
    no_env_file: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inicia los endpoints HTTP de consulta y callbacks de Docling.
    Serve,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if !cli.no_env_file {
        dotenvy::dotenv().ok();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rag=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    match cli.command {
        Command::Serve => rag::serve(config, ServerConfig::from_env()?).await?,
    }
    Ok(())
}
