use anyhow::{Context, Result};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::config::LoggingConfig;

pub fn init(cfg: &LoggingConfig) -> Result<()> {
    // Honor RUST_LOG if set; otherwise use the configured level.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(cfg.level.as_str()));

    let stderr_layer = fmt::layer().with_target(true).with_writer(std::io::stderr);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer);

    if cfg.journald {
        let journald = tracing_journald::layer().context("init journald layer")?;
        registry.with(journald).try_init().context("init tracing")?;
    } else {
        registry.try_init().context("init tracing")?;
    }
    Ok(())
}
