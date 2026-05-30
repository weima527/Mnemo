# Mnemo install + Claude Code wiring (Windows / PowerShell).
#
# Builds the release binary and prints the MCP config snippet to add to
# Claude Code (or any other MCP host) so the host can spawn `mnemo-mcp`
# over stdio. The bridge auto-spawns a detached daemon on first call.
#
# Usage:
#   .\scripts\install.ps1                # build + print config
#   .\scripts\install.ps1 -SkipBuild     # skip cargo (use existing build)

[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path "$PSScriptRoot\..").Path

if (-not $SkipBuild) {
    Write-Host "Building Mnemo (release profile, LTO + opt-level=3)..." -ForegroundColor Cyan
    Write-Host "(first build is slow; subsequent ones are incremental)"
    Push-Location $repo
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}

$bin = Join-Path $repo "target\release\mnemo-mcp.exe"
if (-not (Test-Path $bin)) {
    throw "mnemo-mcp.exe not found at $bin. Build did not produce the binary."
}
$binAbs = (Resolve-Path $bin).Path

Write-Host ""
Write-Host "Mnemo MCP bridge installed:" -ForegroundColor Green
Write-Host "  $binAbs"
Write-Host ""

# Build the JSON snippet with the path JSON-escaped (\ → \\).
$binEscaped = $binAbs -replace '\\', '\\'
$snippet = @"
{
  "mcpServers": {
    "mnemo": {
      "command": "$binEscaped"
    }
  }
}
"@

Write-Host "Add this to your Claude Code MCP config:" -ForegroundColor Yellow
Write-Host ""
Write-Host $snippet -ForegroundColor Cyan
Write-Host ""
Write-Host "Common config paths:"
Write-Host "  Claude Code CLI:  claude mcp add mnemo `"$binAbs`""
Write-Host "  Claude Desktop:   %APPDATA%\Claude\claude_desktop_config.json"
Write-Host ""
Write-Host "Then restart Claude Code. Three tools will appear:"
Write-Host "  - index_repo     (call this first per repository)"
Write-Host "  - trace_symbol   (callers/callees of a symbol)"
Write-Host "  - find_context   (ranked Context Pack for a task)"
Write-Host ""
Write-Host "All data lives in ~/.mnemo/ ; your repos are never written to."
