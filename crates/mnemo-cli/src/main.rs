//! Mnemo CLI — local debugging and benchmarking tool.
//!
//! Usage:
//! ```bash
//! cargo run -p mnemo-cli -- index --repo-path /path/to/repo
//! cargo run -p mnemo-cli -- parse --file src/main.rs
//! cargo run -p mnemo-cli -- bench
//! ```

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Mnemo: local-first code intelligence layer.
#[derive(Parser)]
#[command(name = "mnemo", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index a repository.
    Index {
        /// Path to the repository.
        #[arg(short, long)]
        repo_path: PathBuf,
        /// Force a full re-index (ignore incremental cache).
        #[arg(short, long)]
        force: bool,
    },
    /// Parse a single file and print extracted symbols.
    Parse {
        /// Path to the source file.
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Show version info.
    Version,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Index { repo_path, force } => {
            tracing::info!(path = %repo_path.display(), force, "indexing repository");
            let result = mnemo_index::index_repo(&repo_path, force)?;
            println!("Indexed {} files, {} symbols, {} edges in {}ms (v{})",
                result.file_count,
                result.symbol_count,
                result.edge_count,
                result.elapsed_ms,
                result.index_version,
            );
        }
        Command::Parse { file } => {
            let source = std::fs::read_to_string(&file)?;
            let result = mnemo_parser::parse_file(
                mnemo_core::types::FileId::new_v4(),
                &file,
                &source,
            );
            println!("Language: {:?}", result.language);
            println!("Symbols found: {}", result.symbols.len());
            for sym in &result.symbols {
                println!("  {:?} {} (line {})", sym.kind, sym.name, sym.definition_range.start_line);
            }
            if !result.errors.is_empty() {
                for err in &result.errors {
                    eprintln!("  Parse error at {}:{}: {}", err.line, err.column, err.message);
                }
            }
        }
        Command::Version => {
            println!("mnemo-cli {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
