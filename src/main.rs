//! Entry point for the whisper_input utility.

mod app;
mod audio;
mod config;
mod hotkey;
mod model;
mod paste;
mod sound;
mod tool;
mod transcribe;

use anyhow::Result;
use clap::Parser;
use config::{Cli, Config};
use tracing_subscriber::EnvFilter;

/// Parses configuration, initializes logging, and runs the app loop.
fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();
    let config = Config::from_cli(cli.clone())?;

    if cli.tool_transcribe_once {
        return tool::run_transcribe_once(&config, &cli);
    }

    app::run(config)
}

/// Configures structured logs with an overridable `RUST_LOG` filter.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}
