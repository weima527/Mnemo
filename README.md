<p align="center">
  <img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License">
  <img src="https://img.shields.io/badge/rust-stable-brightgreen.svg" alt="Rust">
  <img src="https://img.shields.io/badge/MCP-ready-8A2BE2.svg" alt="MCP Ready">
  <img src="https://img.shields.io/badge/status-MVP--in--progress-yellow.svg" alt="Status">
</p>

# Mnemo

> **Don't send your agent the whole repository. Send the smallest sufficient context.**

Mnemo is a **local-first, agent-agnostic code intelligence MCP server**. It builds a symbolic graph of your repository, tracks changes incrementally, and returns minimal **Context Packs** — not raw file dumps — to any MCP-compatible coding agent.

---

## Why Mnemo?

Modern coding agents (Claude Code, Codex, Cursor, OpenCode) need context. Today, most tools either:

- Dump entire files into the prompt (wastes tokens, hits context limits).
- Run naive grep/vector search (misses structural relationships).
- Require cloud services (privacy risk, latency, cost).

Mnemo takes a different path:

- **Structural understanding first** — AST-level symbols and edges, not just text.
- **Incremental by default** — only changed files are re-parsed.
- **PR-aware overlays** — analyze a branch without duplicating the index.
- **Local-only** — no cloud, no telemetry, no API keys needed.
- **Agent-agnostic** — one server serves Claude, Codex, Cursor, VS Code, JetBrains.

---

## How It Works

```
User asks agent: "Why did the last PR break the auth module?"

Agent calls Mnemo explain_pr tool
        │
        v
┌──────────────────────────────────────────┐
│  Mnemo MCP Server                        │
│                                          │
│  1. Git diff base..head                  │
│  2. Build overlay on base snapshot       │
│  3. Traverse symbol graph (callers/      │
│     callees of changed symbols)          │
│  4. Score & rank affected context        │
│  5. Return minimal Context Pack          │
│     within token budget                  │
└──────────────────────────────────────────┘
        │
        v
Agent receives structured context:
  - Changed symbols: 3
  - Affected callers: 7
  - Likely test files: 2
  - Risk: Medium
  - Token estimate: 2,140 / 5,000 budget
```

---

## Features

- **Symbol extraction**: Functions, structs, enums, traits, modules via tree-sitter (Rust first, TypeScript next).
- **Call graph**: Traverse callers/callees, imports, module containment.
- **Incremental indexing**: File-content hashing + git diff = re-parse only what changed.
- **MVCC overlays**: PR / working-tree changes layered on base snapshot.
- **Context Pack**: Ranked, budget-aware output with provenance for every item.
- **Memory layers**: Project constitution, user preferences, outcome learning.
- **MCP-native**: Stdio transport, standard tool schemas, works with any MCP host.

---

## MCP Tools

| Tool | What It Does |
|------|-------------|
| `index_repo` | Build or refresh the local code index |
| `find_context` | Given a task, return a minimal Context Pack |
| `trace_symbol` | Follow definitions, references, callers, and callees |
| `impact_analysis` | Predict what a code change will break |
| `explain_pr` | Summarize a PR diff with affected subgraph |

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs) stable (1.75+)
- Git (for repository operations)

### Install & Run

```bash
# Clone
git clone https://github.com/mnemo/mnemo.git
cd mnemo

# Build
cargo build --release

# Index your repo
cargo run -p mnemo-cli -- index --repo-path /path/to/your/project

# Start the MCP server (connects to agent host via stdio)
cargo run -p mnemo-mcp
```

### Configure Your Agent

Add Mnemo to your MCP host config:

```json
{
  "mcpServers": {
    "mnemo": {
      "command": "cargo",
      "args": ["run", "-p", "mnemo-mcp", "--release"],
      "cwd": "/path/to/mnemo"
    }
  }
}
```

Works with: **Claude Code**, **Codex**, **Cursor**, **VS Code** (via MCP extension), **JetBrains**.

---

## Architecture

```
mnemo-cli ───────────────────────────────────┐
                                              │
mnemo-mcp ──┬── mnemo-index ──┬── mnemo-parser ── mnemo-core
            │                 ├── mnemo-store        (types, errors)
            │                 ├── mnemo-graph
            │                 └── mnemo-git
            │
            └── mnemo-memory ── mnemo-store
```

| Crate | Purpose |
|-------|---------|
| `mnemo-core` | Domain types, errors, shared traits |
| `mnemo-parser` | Tree-sitter adapters, symbol extraction |
| `mnemo-store` | Embedded SQLite storage, schema, migrations |
| `mnemo-graph` | Symbol graph, dependency edges, traversal |
| `mnemo-git` | Git diff, refs, working-tree overlay |
| `mnemo-index` | Repository indexing, incremental updates |
| `mnemo-mcp` | MCP server entrypoint, tool registration |
| `mnemo-memory` | Project constitution, outcome memory |
| `mnemo-cli` | Local debugging and benchmarking CLI |

---

## Roadmap

| Phase | Goal |
|-------|------|
| **MVP** (current) | Core graph, 5 MCP tools, Rust + TypeScript parsing |
| **v0.2** | Working tree-sitter extraction, edge construction |
| **v0.3** | Context Pack planner with ranking |
| **v0.4** | Real MCP transport, Claude Code integration |
| **v0.5** | Outcome memory, token savings dashboard |
| **v1.0** | Multi-language, enterprise deployment docs |

---

## Contributing

Mnemo is in early development. Contributions are welcome.

1. Read [`AGENTS.md`](AGENTS.md) — the project constitution.
2. Pick an issue or open one for discussion.
3. Keep changes small and reviewable.
4. Add tests with every behavior change.
5. Preserve local-first, deterministic behavior.

See [`docs/architecture.md`](docs/architecture.md) for technical design.

---

## Principles

- **Local-first** — no cloud dependencies, no telemetry.
- **Graph-first** — structural signals before embeddings.
- **Incremental** — re-index only what changed.
- **Minimal sufficient context** — Context Packs, not raw dumps.
- **Agent-agnostic** — serves any MCP-compatible host.
- **Privacy-respecting** — your code never leaves your machine.

---

## License

Apache 2.0 — see [LICENSE](LICENSE) for full text.

---

<p align="center">
  <sub>Built with Rust · Powered by tree-sitter · Served over MCP</sub>
</p>
