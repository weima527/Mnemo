# M2 Exit Demo (manual smoke checklist)

PLAN.md's M2 demo lists five behaviours the multi-tenant daemon stack
needs to support. Sprint 1 + Sprint 3 covered the implementation
(daemon, IPC, MCP bridge, overlay + watcher); the e2e integration test
(`crates/mnemo-mcp/tests/e2e_stdio.rs`) covers the protocol hop. The
checks below are the **operational** behaviours that can't easily be
asserted in a `cargo test` — a real terminal session is the cleanest
way to confirm them.

Run them once after building, and after any change that touches IPC,
LRU, watcher, or daemon shutdown.

## Prerequisites

```bash
cargo build --release
```

Binaries: `target/release/mnemo` (CLI), `target/release/mnemo-daemon`,
`target/release/mnemo-mcp`. The CLI is the easiest probe; the e2e test
already covers the MCP-over-stdio path.

Throughout: replace `/path/to/repoX` with real repo paths (each just
needs to have some `.rs` or `.ts` files).

## 1. Daemon resident, 3+ projects concurrent

```bash
# Terminal A
./target/release/mnemo daemon start --foreground

# Terminal B
./target/release/mnemo project attach /path/to/repo-a
./target/release/mnemo project attach /path/to/repo-b
./target/release/mnemo project attach /path/to/repo-c
./target/release/mnemo daemon status
```

Expect: `Active projects: 3 / 5`, each line listing one project_id +
canonical_path + symbol_count.

## 2. LRU eviction (memory stays bounded)

```bash
# Default max_active_projects is 5. Attach a 6th project.
./target/release/mnemo project attach /path/to/repo-d
./target/release/mnemo project attach /path/to/repo-e
./target/release/mnemo project attach /path/to/repo-f   # triggers eviction
./target/release/mnemo daemon status
```

Expect: still 5 active projects. The least-recently-used one (repo-a,
unless touched) was evicted. **Its DB on disk is untouched** —
`ls ~/.mnemo/projects/` still shows all six directories.

```bash
# Re-attach the evicted project; it lazily re-hydrates from its DB.
./target/release/mnemo project attach /path/to/repo-a
./target/release/mnemo daemon status   # still 5 active, different LRU tail
```

## 3. Overlay watcher: live edits reflected in queries

(Requires M3.2's file watcher; auto-started on attach.)

```bash
# Index repo-a, then ask for a known symbol.
./target/release/mnemo index --repo-path /path/to/repo-a
./target/release/mnemo query symbol some_function --repo-path /path/to/repo-a

# Edit some file in repo-a: add a new function, save.
# Wait ~3 seconds (2s debounce + a little slack).

# Ask again — the new function shows up without re-indexing.
./target/release/mnemo query symbol new_function --repo-path /path/to/repo-a
```

Expect: the new symbol is found. **No write into the project dir**
(`ls /path/to/repo-a/.mnemo` returns "no such file"). The overlay is
purely in memory.

## 4. `kill -9 mnemo-daemon` → bridge auto-restarts

```bash
# In another terminal, find the daemon PID.
pgrep mnemo-daemon          # Linux/macOS
Get-Process mnemo-daemon    # PowerShell

# Hard-kill it.
kill -9 <pid>               # Linux/macOS
Stop-Process -Force <pid>   # PowerShell
```

Now invoke any tool — the CLI client retries transient connection
failures (up to 10 attempts, ~1.1s budget); the MCP bridge spawns a
fresh detached daemon on first call. Either way the next operation
succeeds:

```bash
./target/release/mnemo query symbol some_function --repo-path /path/to/repo-a
# Or via Claude Code: ask it to call find_context.
```

Expect: the operation returns normally. The new daemon re-hydrates the
project on demand (the DB on disk is the source of truth).

## 5. MCP host end-to-end (covered by the integration test)

This used to be the long-standing M2.3 caveat ("bridge→daemon logic
tested, stdio MCP itself never validated from a real host"). It's now
closed automatically:

```bash
cargo test --workspace mcp_stdio_e2e_three_tools
```

The test spawns the real `mnemo-mcp` binary, sends an MCP initialize +
tools/list + tools/call(index_repo) + tools/call(find_context) over
line-delimited JSON-RPC on stdio, and asserts that the Context Pack
items include the task's anchor symbols.

For interactive validation with the actual Claude Code:

1. Run `scripts/install.{ps1,sh}` to build and print the config snippet.
2. Add the snippet to your Claude Code MCP config.
3. Restart Claude Code; the three tools appear in the tool palette.
4. Ask Claude to call `index_repo` on a known repo, then `find_context`
   on a task. The ranked items appear in Claude's response.

See [`docs/claude-code.md`](claude-code.md) for the full setup.

## When something fails

- **Daemon won't start**: `~/.mnemo/` not writable? Permission issue?
- **`Active projects` doesn't grow**: each `project attach` returns its
  `project_id` — check stderr for an error.
- **Eviction kept the wrong project**: `last_accessed` is bumped on
  every query. Make sure you didn't accidentally touch the one you
  expected to be evicted.
- **Live edits don't reflect**: the watcher debounces ~2s. Wait a bit
  longer. Also check `tracing` logs from the daemon (it warns when
  `set_overlay` fails).
- **`kill -9` and then nothing recovers**: confirm the bridge / CLI is
  actually trying to reconnect (look at `cargo run -p mnemo-cli -- ...
  -v` output). The retry loop has a 1.1s budget; one slow round won't
  exhaust it but ten back-to-back failures will.
