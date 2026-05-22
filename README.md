# Mnemo

Local-first, agent-agnostic code intelligence layer.

Mnemo is an MCP Server that builds a lightweight code graph, maintains incremental
repository indexes, supports PR / working-tree overlays, and returns the smallest
sufficient **Context Pack** for coding agents.

## Architecture

```
Agent / IDE / CLI
      |
      v
Rust MCP Server
      |
      +-- Repo Indexer
      +-- Symbol Graph
      +-- MVCC Overlay Layer
      +-- Context Pack Planner
      +-- Memory Layer
```

## Crates

| Crate | Purpose |
|-------|---------|
| `mnemo-core` | Domain types, errors, traits |
| `mnemo-parser` | Tree-sitter adapters, symbol extraction |
| `mnemo-store` | Embedded SQLite storage, schema, migrations |
| `mnemo-graph` | Symbol graph, dependency edges, traversal |
| `mnemo-git` | Git diff, refs, working-tree overlay |
| `mnemo-index` | Repository indexing, incremental updates |
| `mnemo-mcp` | MCP server entrypoint, tool registration |
| `mnemo-memory` | Project constitution, outcome memory |
| `mnemo-cli` | Local debugging and benchmarking CLI |

## Quick Start

```bash
# Install Rust (stable)
# Build everything
cargo build

# Run the CLI
cargo run -p mnemo-cli -- --help

# Start the MCP server
cargo run -p mnemo-mcp
```

## Principles

- **Local-first** — no cloud dependencies.
- **Graph-first** — structural signals before embeddings.
- **Incremental** — only changed files trigger re-indexing.
- **Minimal sufficient context** — Context Packs, not raw file dumps.
- **Agent-agnostic** — serves any MCP-compatible host.

## License

TBD
