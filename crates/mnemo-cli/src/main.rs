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
    /// Query the indexed graph.
    Query {
        #[command(subcommand)]
        query: QueryCommand,
    },
    /// Show version info.
    Version,
}

#[derive(Subcommand)]
enum QueryCommand {
    /// Symbols that call NAME.
    Callers {
        /// Symbol name to look up.
        name: String,
        /// Repository to query.
        #[arg(short, long, default_value = ".")]
        repo_path: PathBuf,
    },
    /// Symbols that NAME calls.
    Callees {
        /// Symbol name to look up.
        name: String,
        /// Repository to query.
        #[arg(short, long, default_value = ".")]
        repo_path: PathBuf,
    },
    /// Symbols whose (qualified) name contains PATTERN.
    Search {
        /// Case-insensitive substring to match.
        pattern: String,
        /// Repository to query.
        #[arg(short, long, default_value = ".")]
        repo_path: PathBuf,
    },
    /// Show details for symbols named NAME.
    Symbol {
        /// Symbol name to look up.
        name: String,
        /// Repository to query.
        #[arg(short, long, default_value = ".")]
        repo_path: PathBuf,
    },
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
            println!("project:  {}", result.project_id);
            println!("index db: {}", result.db_path.display());
            println!(
                "Indexed {} files, {} new symbols, {} new edges in {}ms (snapshot={}, schema v{})",
                result.file_count,
                result.symbol_count,
                result.edge_count,
                result.elapsed_ms,
                result.snapshot,
                result.index_version,
            );
        }
        Command::Parse { file } => {
            let source = std::fs::read_to_string(&file)?;
            let result = mnemo_parser::parse_file(
                mnemo_core::FileIdentityId::ZERO, // temporary until M0.4 path resolution
                &file,
                &source,
            );
            println!("Language: {:?}", result.language);
            println!("Symbols found: {}", result.symbols.len());
            for sym in &result.symbols {
                println!(
                    "  {:?} {} (line {})",
                    sym.kind, sym.qualified_name, sym.definition_range.start_line
                );
            }
            println!("Edges found: {}", result.edges.len());
            for edge in &result.edges {
                println!(
                    "  {:?} {} -> {}",
                    edge.kind, edge.from_qualified_name, edge.to_name
                );
            }
            if !result.errors.is_empty() {
                for err in &result.errors {
                    eprintln!("  Parse error at {}:{}: {}", err.line, err.column, err.message);
                }
            }
        }
        Command::Query { query } => run_query(query)?,
        Command::Version => {
            println!("mnemo-cli {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}

fn run_query(query: QueryCommand) -> anyhow::Result<()> {
    match query {
        QueryCommand::Callers { name, repo_path } => {
            print_relations(&name, &mnemo_index::query::callers(&repo_path, &name)?, "callers");
        }
        QueryCommand::Callees { name, repo_path } => {
            print_relations(&name, &mnemo_index::query::callees(&repo_path, &name)?, "callees");
        }
        QueryCommand::Search { pattern, repo_path } => {
            let hits = mnemo_index::query::search(&repo_path, &pattern)?;
            if hits.is_empty() {
                println!("no symbols matching {pattern:?}");
            }
            for hit in hits {
                println!(
                    "{:<28} {:?}  {}:{}",
                    hit.qualified_name, hit.kind, hit.file_path, hit.start_line
                );
            }
        }
        QueryCommand::Symbol { name, repo_path } => {
            let hits = mnemo_index::query::symbol_info(&repo_path, &name)?;
            if hits.is_empty() {
                println!("no symbol named {name:?} (is the project indexed?)");
            }
            for hit in hits {
                println!(
                    "{} ({:?})  {}:{}",
                    hit.qualified_name, hit.kind, hit.file_path, hit.start_line
                );
            }
        }
    }
    Ok(())
}

fn print_relations(name: &str, reports: &[mnemo_index::query::Relations], label: &str) {
    if reports.is_empty() {
        println!("no symbol named {name:?} (is the project indexed?)");
        return;
    }
    for report in reports {
        println!(
            "{} ({}:{})",
            report.symbol.qualified_name, report.symbol.file_path, report.symbol.start_line
        );
        println!("  {label}:");
        if report.related.is_empty() {
            println!("    (none)");
        } else {
            for rel in &report.related {
                println!(
                    "    - {} ({}:{})",
                    rel.qualified_name, rel.file_path, rel.start_line
                );
            }
        }
    }
}
