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

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create snapshots and prune according to the retention policy.
    Snapshot {
        /// Show what would happen without touching the filesystem.
        #[arg(long)]
        dry_run: bool,

        /// Print the plan or run results as JSON.
        #[arg(long)]
        json: bool,

        /// Restrict to these target paths (default: all configured targets).
        #[arg(value_name = "TARGET")]
        targets: Vec<PathBuf>,
    },
    /// List existing snapshots.
    List {
        /// Print as JSON.
        #[arg(long)]
        json: bool,

        /// Restrict to these target paths (default: all configured targets).
        #[arg(value_name = "TARGET")]
        targets: Vec<PathBuf>,
    },
    /// Delete individual snapshots (asks for confirmation).
    Delete {
        /// Show what would be deleted without touching the filesystem.
        #[arg(long)]
        dry_run: bool,

        /// Target path (must be configured).
        #[arg(value_name = "TARGET")]
        target: PathBuf,

        /// Snapshot names, e.g. daily-20260101.
        #[arg(value_name = "NAME", required = true)]
        names: Vec<String>,
    },
    /// Restore a file or subtree from a snapshot into a destination.
    Restore {
        /// Target path (must be configured).
        #[arg(value_name = "TARGET")]
        target: PathBuf,

        /// Snapshot name, e.g. daily-20260101.
        #[arg(value_name = "NAME")]
        name: String,

        /// Path inside the snapshot to restore (default: the whole snapshot).
        #[arg(long, value_name = "SUBPATH")]
        path: Option<PathBuf>,

        /// Destination to copy into (required; nothing is overwritten in place).
        #[arg(long, value_name = "DEST", required = true)]
        to: PathBuf,
    },
}

fn main() -> Result<()> {
    bonsai::logger::init();
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Command::Snapshot {
            dry_run,
            json,
            targets,
        } => app::snapshot(&config, dry_run, &targets, json),
        Command::List { json, targets } => app::list(&config, &targets, json),
        Command::Delete {
            dry_run,
            target,
            names,
        } => app::delete(&config, &target, &names, dry_run),
        Command::Restore {
            target,
            name,
            path,
            to,
        } => app::restore(&config, &target, &name, path.as_deref(), &to),
    }
}
