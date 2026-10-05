use rag::{Config, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rag=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    rag::serve(config, ServerConfig::from_env()?).await?;
    Ok(())
}
