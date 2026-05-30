# Using Mnemo with Claude Code (or any MCP host)

Mnemo ships an MCP server, `mnemo-mcp`, that any MCP-compatible host can
spawn over stdio. It exposes three tools backed by a resident daemon:

| Tool | What it does |
|---|---|
| `index_repo` | Index (or refresh) a repository. **Call this first per repo.** |
| `trace_symbol` | Callers / callees of a symbol by name. |
| `find_context` | Build a ranked, token-budgeted Context Pack for a task. |

A working-tree file watcher keeps the in-memory overlay in sync with your
disk edits on a ~2-second debounce — `find_context` reflects unsaved
changes without explicit overlay calls.

## 1. Build

The one-click script builds the release binary and prints the exact MCP
config snippet to paste:

```powershell
# Windows / PowerShell
.\scripts\install.ps1
```

```bash
# Linux / macOS
./scripts/install.sh
```

Or manually:

```bash
cargo build --release
# binary at: target/release/mnemo-mcp  (or mnemo-mcp.exe on Windows)
```

First build is slow (LTO + opt-level=3). Subsequent builds are incremental.

## 2. Register with Claude Code

### Via the Claude Code CLI

```bash
claude mcp add mnemo /absolute/path/to/mnemo-mcp
```

### Or by editing the config JSON directly

Add the `mnemo` block to `mcpServers` in your Claude Code MCP config. A
copy-pastable template is in [`mcp_config.example.json`](../mcp_config.example.json).

```json
{
  "mcpServers": {
    "mnemo": {
      "command": "/absolute/path/to/mnemo-mcp"
    }
  }
}
```

Restart Claude Code so it picks up the new server. The three tools should
appear in the tool list.

## 3. First call

In a Claude Code session, ask Claude to use the tools:

> Use `index_repo` on `/path/to/my-repo`.
>
> Then use `find_context` for "fix the auth bug" in the same repo.

The bridge auto-spawns a detached `mnemo-daemon` on the first call; later
calls reuse the same daemon.

## Where Mnemo writes

| Path | Contents |
|---|---|
| `~/.mnemo/registry.db` | Global project registry. |
| `~/.mnemo/projects/<id>/index.db` | Per-project MVCC index. |
| `~/.mnemo/daemon.sock` (Unix) | IPC endpoint. |
| `\\.\pipe\mnemo-daemon` (Windows) | IPC endpoint. |

**Your repos are never written to** — Mnemo's zero-pollution rule
(DESIGN §3.2 / §12 #2).

## Troubleshooting

- **"No such tool" / tools don't appear.** Restart Claude Code after editing
  the config. Confirm the binary path is absolute and executable.
- **Bridge hangs / no response.** `mnemo-mcp`'s logs go to stderr (visible in
  Claude Code's MCP server panel). Also check that the daemon endpoint isn't
  blocked by an OS firewall rule on the named pipe.
- **Daemon won't start.** Run it manually to see logs:
  `mnemo daemon start --foreground`. Check `~/.mnemo/` exists and is
  writable.
- **Indexing finds zero symbols.** Confirm the repo has Rust / TypeScript
  files (the only languages M3 supports). `target/`, `node_modules/`, `.git/`
  are ignored by default.

## Status (M3 productization stage)

| Feature | Status |
|---|---|
| 3 MCP tools (index_repo / trace_symbol / find_context) | ✅ |
| Cross-platform IPC daemon (Linux / macOS / Windows) | ✅ |
| Working-tree overlay + automatic file watcher | ✅ M3.2 |
| Context Pack ranking + usefulness feedback loop | ✅ M3.1 + M3.9 |
| Background snapshot GC | ✅ M3.10 |
| TypeScript / TSX / JS extraction | ✅ M3.8 |
| `impact_analysis` / `explain_pr` tools | ⏳ deferred (DESIGN §7.4 / §7.5) |
| Git HEAD watcher → auto commit snapshot | ⏳ Sprint 1 follow-up |
| Memory layer (constitution / preference / outcome / skill / session) | ⏳ v1.0 (DESIGN §2.6) |

See `PLAN.md` and `DESIGN.md` for the full roadmap.
