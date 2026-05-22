# Mnemo Architecture

## Overview

Mnemo is a local-first MCP server that provides code intelligence to coding agents.
It builds a symbolic graph of a repository, tracks changes incrementally, and
returns minimal sufficient **Context Packs** rather than raw file dumps.

## Crate Map

```text
mnemo-cli ──────────────────────────────────────────┐
                                                     │
mnemo-mcp ──┬── mnemo-index ──┬── mnemo-parser ── mnemo-core
            │                 ├── mnemo-store
            │                 ├── mnemo-graph
            │                 └── mnemo-git
            │
            └── mnemo-memory ── mnemo-store ────── mnemo-core
```

## Data Flow

1. **Indexing** (`index_repo`):
   - Walk file tree → hash files → detect changes
   - Parse changed files → extract symbols and edges
   - Persist to SQLite → update in-memory graph

2. **Context Planning** (`find_context`):
   - Parse task → select anchor symbols
   - Traverse graph (callers, callees, imports)
   - Score and rank candidates
   - Pack within token budget → return Context Pack

3. **Impact Analysis** (`impact_analysis`):
   - Given a diff or symbol change
   - Traverse dependency graph outward
   - Report affected symbols, files, and likely tests

4. **PR Explanation** (`explain_pr`):
   - Diff two git refs
   - Build overlay on base snapshot
   - Generate change summary + affected subgraph

## Storage

- SQLite (via `rusqlite`, bundled) in `.mnemo/index.db`
- WAL mode for concurrent reads
- Schema versioned with incremental migrations

## Invariants

- All file I/O is constrained to the configured repository root
- Indexes are stored alongside the repo (`.mnemo/`)
- No network access in MVP
- Symbol graph is structural (AST-based), not embedding-based
