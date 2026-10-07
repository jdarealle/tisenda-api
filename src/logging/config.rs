use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Development,
    Production,
}

impl Mode {
    pub(super) fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Development
        } else {
            Self::Production
        }
    }

    pub(super) fn filter(self, value: Option<&str>) -> Result<EnvFilter> {
        let default = match self {
            Self::Development => "rag=debug",
            Self::Production => "rag=info",
        };
        EnvFilter::builder()
            .with_regex(false)
            .parse(
                value
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(default),
            )
            .map_err(|_| anyhow::anyhow!("RUST_LOG contiene un filtro inválido"))
    }

    pub(super) fn ansi(self, terminal: bool, no_color: bool) -> bool {
        self == Self::Development && terminal && !no_color
    }
}
