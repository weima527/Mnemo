#!/usr/bin/env bash
# Mnemo install + Claude Code wiring (Linux / macOS).
#
# Builds the release binary and prints the MCP config snippet to add to
# Claude Code (or any other MCP host) so the host can spawn `mnemo-mcp`
# over stdio. The bridge auto-spawns a detached daemon on first call.
#
# Usage:
#   ./scripts/install.sh                # build + print config
#   ./scripts/install.sh --skip-build   # skip cargo (use existing build)

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skip_build=0
for arg in "$@"; do
    case "$arg" in
        --skip-build) skip_build=1 ;;
        -h|--help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

if [[ "$skip_build" -eq 0 ]]; then
    echo "Building Mnemo (release profile, LTO + opt-level=3)..."
    echo "(first build is slow; subsequent ones are incremental)"
    (cd "$repo" && cargo build --release)
fi

bin="$repo/target/release/mnemo-mcp"
if [[ ! -x "$bin" ]]; then
    echo "Error: mnemo-mcp not found at $bin. Build did not produce the binary." >&2
    exit 1
fi

# Resolve to an absolute, canonical path.
bin_abs="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"

cat <<EOF

Mnemo MCP bridge installed:
  $bin_abs

Add this to your Claude Code MCP config:

{
  "mcpServers": {
    "mnemo": {
      "command": "$bin_abs"
    }
  }
}

Common config paths:
  Claude Code CLI:  claude mcp add mnemo "$bin_abs"
  Claude Desktop:   ~/Library/Application Support/Claude/claude_desktop_config.json   (macOS)
                    ~/.config/Claude/claude_desktop_config.json                       (Linux)

Then restart Claude Code. Three tools will appear:
  - index_repo     (call this first per repository)
  - trace_symbol   (callers/callees of a symbol)
  - find_context   (ranked Context Pack for a task)

All data lives in ~/.mnemo/ ; your repos are never written to.
EOF
