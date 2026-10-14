use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use bonsai::app;
use bonsai::config::Config;

/// Grandfather-father-son snapshot manager for bcachefs.
#[derive(Debug, Parser)]
#[command(name = "bonsai", version, about, propagate_version = true)]
struct Cli {
    /// Path to the configuration file.
    #[arg(
        short,
        long,
        value_name = "FILE",
        default_value = "/etc/bonsai/config.toml",
        global = true
    )]
    config: PathBuf,

    /// Show what would happen without touching the filesystem.
    #[arg(long, global = true)]
    dry_run: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create snapshots and prune according to the retention policy.
    Snapshot {
        /// Restrict to these target paths (default: all configured targets).
        #[arg(value_name = "TARGET")]
        targets: Vec<PathBuf>,
    },
    /// List existing snapshots.
    List {
        /// Restrict to these target paths (default: all configured targets).
        #[arg(value_name = "TARGET")]
        targets: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    bonsai::logger::init();
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Command::Snapshot { targets } => app::snapshot(&config, cli.dry_run, &targets),
        Command::List { targets } => app::list(&config, &targets),
    }
}
