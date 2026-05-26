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
    /// Manage the resident daemon.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    /// Manage projects in the running daemon.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
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

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run the daemon in the foreground (blocks until Ctrl-C or `daemon stop`).
    Start {
        /// Run in the foreground (the only supported mode for now).
        #[arg(long)]
        foreground: bool,
    },
    /// Show daemon status (active projects, uptime).
    Status,
    /// Ask the running daemon to shut down.
    Stop,
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Attach (hydrate + cache) a project in the daemon.
    Attach {
        /// Path to the project.
        path: PathBuf,
    },
    /// Detach a project (frees memory, keeps the DB).
    Detach {
        /// Path to the project.
        path: PathBuf,
    },
    /// List the daemon's active projects.
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
                    eprintln!(
                        "  Parse error at {}:{}: {}",
                        err.line, err.column, err.message
                    );
                }
            }
        }
        Command::Query { query } => run_query(query)?,
        Command::Daemon { cmd } => run_daemon(cmd).await?,
        Command::Project { cmd } => run_project(cmd).await?,
        Command::Version => {
            println!("mnemo-cli {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}

fn run_query(query: QueryCommand) -> anyhow::Result<()> {
    match query {
        QueryCommand::Callers { name, repo_path } => {
            print_relations(
                &name,
                &mnemo_index::query::callers(&repo_path, &name)?,
                "callers",
            );
        }
        QueryCommand::Callees { name, repo_path } => {
            print_relations(
                &name,
                &mnemo_index::query::callees(&repo_path, &name)?,
                "callees",
            );
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

async fn run_daemon(cmd: DaemonCmd) -> anyhow::Result<()> {
    let endpoint = mnemo_daemon::transport::default_endpoint();
    match cmd {
        DaemonCmd::Start { foreground } => {
            if !foreground {
                println!("detached mode is not supported yet; re-run with --foreground");
                return Ok(());
            }
            println!("mnemo daemon listening on {endpoint} (Ctrl-C to stop)");
            mnemo_daemon::run_default().await?;
        }
        DaemonCmd::Status => {
            let status =
                mnemo_daemon::client::call(&endpoint, "daemon.status", serde_json::json!({}))
                    .await?;
            println!(
                "Active projects: {} / {}  (uptime {}s)",
                status["active_projects"], status["max_active_projects"], status["uptime_secs"]
            );
            if let Some(projects) = status["projects"].as_array() {
                for p in projects {
                    println!(
                        "  {}  {}  ({} symbols)",
                        p["project_id"].as_str().unwrap_or(""),
                        p["canonical_path"].as_str().unwrap_or(""),
                        p["symbol_count"]
                    );
                }
            }
        }
        DaemonCmd::Stop => {
            mnemo_daemon::client::call(&endpoint, "daemon.shutdown", serde_json::json!({})).await?;
            println!("daemon shutdown requested");
        }
    }
    Ok(())
}

async fn run_project(cmd: ProjectCmd) -> anyhow::Result<()> {
    let endpoint = mnemo_daemon::transport::default_endpoint();
    match cmd {
        ProjectCmd::Attach { path } => {
            let info = mnemo_daemon::client::call(
                &endpoint,
                "project.attach",
                serde_json::json!({ "path": path.to_string_lossy() }),
            )
            .await?;
            println!(
                "attached {} ({} symbols)",
                info["project_id"].as_str().unwrap_or(""),
                info["symbol_count"]
            );
        }
        ProjectCmd::Detach { path } => {
            let (id, _) = mnemo_store::paths::resolve_project_id(&path)?;
            mnemo_daemon::client::call(
                &endpoint,
                "project.detach",
                serde_json::json!({ "project_id": id.to_hex() }),
            )
            .await?;
            println!("detached {id}");
        }
        ProjectCmd::List => {
            let list = mnemo_daemon::client::call(&endpoint, "project.list", serde_json::json!({}))
                .await?;
            match list.as_array() {
                Some(arr) if arr.is_empty() => println!("no active projects"),
                Some(arr) => {
                    for p in arr {
                        println!(
                            "{}  {}  ({} symbols)",
                            p["project_id"].as_str().unwrap_or(""),
                            p["canonical_path"].as_str().unwrap_or(""),
                            p["symbol_count"]
                        );
                    }
                }
                None => {}
            }
        }
    }
    Ok(())
}
