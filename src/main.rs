use rag::{Config, ServerConfig};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let _logging = match rag::init_logging() {
        Ok(guard) => guard,
        Err(error) => {
            // Before a subscriber exists, keep bootstrap diagnostics safe and parseable.
            eprintln!(
                "{}",
                serde_json::json!({"level": "ERROR", "fields": {
                    "event": "logging_init_failed", "message": error.to_string()
                }})
            );
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        event = "application_starting",
        version = env!("CARGO_PKG_VERSION")
    );
    let result = async {
        let config = Config::from_env().map_err(|_| "config_invalid")?;
        let server = ServerConfig::from_env().map_err(|_| "server_config_invalid")?;
        rag::serve(config, server)
            .await
            .map_err(|_| "server_failed")
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            tracing::error!(event = "application_failed", error_code = code);
            ExitCode::FAILURE
        }
    }
}
