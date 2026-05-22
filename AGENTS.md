# AGENT.md

## Project: GreenCode Layer

Mnemo is a local-first, agent-agnostic code intelligence layer.

The core product is a Rust-based MCP Server that builds a lightweight code graph, maintains incremental repository indexes, supports PR / working-tree overlays, and returns the smallest sufficient Context Pack for coding agents.

This repository should be developed as infrastructure, not as a full coding agent replacement.

---

## 1. Product Positioning

### What this project is

Mnemo is:

- A local MCP Server for code intelligence.
- A minimal Context Pack generator for coding agents.
- A graph-first repository memory runtime.
- An on-device code understanding substrate that can serve Claude Code, Codex, OpenCode, Cursor, VS Code, JetBrains, and other agent hosts.
- A future MemoryOS-like layer for codebase-specific memory, project constitution, and personalized developer workflow learning.

### What this project is not

Mnemo is not:

- A new LLM.
- A full IDE.
- A Claude Code / Codex / OpenCode replacement.
- A cloud-only RAG service.
- A generic vector-search wrapper.
- A chatbot memory product.

The product must complement existing coding agents by providing better local context and persistent codebase memory.

---

## 2. Core Design Thesis

The system should follow this principle:

> Do not send the largest possible context to the model. Send the smallest sufficient context with provenance.

The first technical moat is not “search code.”  
The first technical moat is:

1. Build a local symbolic graph of the repository.
2. Track changes incrementally.
3. Treat PRs / working-tree changes as MVCC-style overlays.
4. Extract a bounded affected subgraph.
5. Generate a minimal Context Pack.
6. Record outcome signals so the system becomes more project-specific over time.

---

## 3. MVP Scope

The MVP should focus only on the local Rust MCP Server and a small number of high-value tools.

### Required MVP tools

The first MCP tool surface should include:

```text
index_repo
find_context
trace_symbol
impact_analysis
explain_pr
```

### Tool responsibilities

#### `index_repo`

Build or refresh the local repository index.

Inputs:

- repository path
- optional language filter
- optional force rebuild flag

Outputs:

- indexed file count
- symbol count
- edge count
- elapsed time
- index version

#### `find_context`

Given a natural language task or coding request, return a minimal Context Pack.

Inputs:

- task text
- repository path
- optional current file
- optional token budget
- optional changed files

Outputs:

- ranked files / symbols
- code ranges
- reason for inclusion
- estimated token count
- provenance

#### `trace_symbol`

Trace a symbol through definitions, references, callers, callees, and related files.

Inputs:

- symbol name
- optional file path
- optional direction: callers / callees / both
- optional max depth

Outputs:

- matched symbol
- related symbols
- graph edges
- source ranges
- confidence

#### `impact_analysis`

Analyze the likely impact of a change.

Inputs:

- file path or symbol
- optional diff
- optional max graph depth

Outputs:

- affected symbols
- affected files
- likely tests
- risk category
- explanation

#### `explain_pr`

Analyze a PR or local diff and return a concise explanation.

Inputs:

- base ref
- head ref
- repository path
- optional token budget

Outputs:

- change summary
- affected subgraph
- important files
- likely tests
- Context Pack
- risk notes

---

## 4. Preferred Architecture

Use a layered architecture.

```text
mnemo /
  crates/
    mnemo-mcp/              # MCP server entrypoint and tool registration
    mnemo-core/             # domain model, context planner, scoring
    mnemo-index/            # repository indexing, file hashing, incremental updates
    mnemo-parser/           # tree-sitter adapters and symbol extraction
    mnemo-graph/            # symbol graph, dependency graph, traversal
    mnemo-git/              # git diff, refs, working-tree overlay
    mnemo-store/            # embedded storage, schema, migrations
    mnemo-memory/           # project constitution, user profile, outcome memory
    mnemo-cli/              # local CLI for debugging and benchmarking
  examples/
  tests/
  docs/
  benches/
```

If the repository is still small, do not over-split too early. Start with fewer crates if needed, but preserve clear module boundaries.

---

## 5. Runtime Architecture

The MVP runtime should look like this:

```text
Agent / IDE / CLI
      |
      v
Rust MCP Server
      |
      +-- Repo Indexer
      |     +-- Git watcher / diff detector
      |     +-- File hash tracker
      |     +-- Tree-sitter parser
      |
      +-- Symbol Graph
      |     +-- symbols
      |     +-- definitions
      |     +-- references
      |     +-- imports
      |     +-- call edges
      |     +-- dependency edges
      |
      +-- MVCC Overlay Layer
      |     +-- base snapshot
      |     +-- working-tree overlay
      |     +-- PR overlay
      |
      +-- Context Pack Planner
      |     +-- anchor selection
      |     +-- graph expansion
      |     +-- ranking
      |     +-- token budget enforcement
      |
      +-- Memory Layer
            +-- project constitution
            +-- user preferences
            +-- outcome telemetry
            +-- learned repo facts
```

---

## 6. Technical Principles

### 6.1 Local-first

All raw code, index data, memory, and telemetry must stay local by default.

Do not add cloud dependencies in the MVP.

### 6.2 Graph-first, vector-later

The initial design should prioritize symbolic and structural signals:

- AST nodes
- symbol definitions
- references
- import edges
- call edges
- module dependencies
- test proximity
- git changes

Do not start with embeddings as the primary retrieval mechanism.

Vectors may be added later for:

- comments
- design docs
- issues
- PR descriptions
- architecture notes
- ambiguous natural-language queries

### 6.3 Incremental by default

Avoid full-repo reindexing unless explicitly requested.

Use:

- file content hashes
- git diff
- git status
- parser version
- index schema version

Only changed files and their affected graph neighborhoods should be recomputed.

### 6.4 MVCC-style PR overlays

A PR or working-tree change should not duplicate the entire repository graph.

Use:

```text
base_snapshot + overlay_delta
```

Reads should resolve as:

```text
overlay first, base second
```

This allows:

- fast PR analysis
- rollback
- parallel branch analysis
- deterministic provenance

### 6.5 Context Pack, not raw search dump

The output to the agent should be a structured Context Pack.

A Context Pack should include:

- task summary
- selected files
- selected symbols
- exact source ranges
- reason for each selected item
- graph relationship summary
- estimated token count
- omitted-but-related items if useful
- provenance

Do not return entire files unless necessary.

---

## 7. Context Pack Scoring

The planner should rank candidate nodes using an explicit scoring model.

Initial heuristic:

```text
score(node) =
    task_anchor_match
  + diff_overlap
  + symbol_name_match
  + callgraph_proximity
  + dependency_proximity
  + test_proximity
  + ownership_or_hotspot_signal
  + historical_usefulness
  - token_cost
  - stale_or_low_confidence_penalty
```

The first MVP can use static weights.

Later versions may use a local contextual bandit or adaptive ranking policy.

---

## 8. Memory Model

The project should evolve from retrieval to memory.

### 8.1 Memory layers

Use three memory layers:

```text
1. Project Constitution Memory
2. User / Team Preference Memory
3. Outcome Memory
```

#### Project Constitution Memory

Stable repo facts, such as:

- build commands
- test commands
- architecture boundaries
- important modules
- generated-code paths
- forbidden edit paths
- code style rules
- migration rules

This layer should be small, explicit, and human-editable.

#### User / Team Preference Memory

Developer or team preferences, such as:

- preferred explanation depth
- preferred review style
- risk tolerance
- favorite test commands
- ignored paths
- coding conventions

This layer must remain local by default.

#### Outcome Memory

Observed signals from previous runs, such as:

- which Context Packs were useful
- which files the agent later had to read
- which tests failed after changes
- which modules often co-change
- which reviewer comments repeat
- which suggestions were accepted or rejected

Outcome memory should store provenance, confidence, timestamp, and decay policy.

### 8.2 Memory promotion rule

Do not automatically promote noisy observations into stable memory.

Use this promotion policy:

```text
raw event -> candidate memory -> confirmed memory -> project constitution
```

Only stable and repeated facts should be promoted.

---

## 9. Storage Requirements

For MVP, prefer embedded storage.

Recommended options:

- SQLite for graph tables, FTS, metadata, events, and memory.
- Optional `sled` or `redb` only if there is a clear performance or packaging reason.
- Avoid external databases in MVP.

Suggested logical tables:

```text
repo
file
file_version
symbol
symbol_version
edge
edge_version
snapshot
overlay
context_pack
context_pack_item
memory_fact
memory_event
tool_run
telemetry_event
```

Each stored fact should include provenance where possible:

```text
source_file
source_range
git_commit
file_hash
parser_version
created_at
confidence
```

---

## 10. Rust Coding Guidelines

### 10.1 General Rust style

Use safe Rust by default.

Avoid `unsafe` unless:

- there is a strong measured reason;
- the safety invariant is documented;
- the code is covered by tests.

Prefer:

- explicit error types
- small modules
- deterministic behavior
- clear ownership
- zero-copy only where it does not damage readability

### 10.2 Error handling

Use `thiserror` for domain errors.

Use `anyhow` only at binary boundaries, CLI entrypoints, or integration layers.

Library crates should expose typed errors.

### 10.3 Async

Use async only where it is needed:

- MCP transport
- filesystem watching
- external process calls
- long-running background indexing

Do not make internal graph algorithms async unnecessarily.

### 10.4 Logging and tracing

Use `tracing`.

Required spans:

- MCP tool call
- repo indexing
- parsing
- graph update
- context planning
- storage query
- memory update

### 10.5 Resource control

The system must run on a normal laptop.

Design targets for MVP:

```text
Idle memory:        < 150 MB for small/medium repos
Indexing memory:    bounded and observable
Incremental update: only changed files
Context response:   target < 5s for medium repos
Network access:     disabled by default
```

These are targets, not strict guarantees in early development.

---

## 11. MCP Design Rules

### 11.1 Keep tools small

Do not expose too many MCP tools initially.

Each tool should have:

- clear input schema
- clear output schema
- bounded output size
- predictable execution behavior
- provenance

### 11.2 Progressive disclosure

Return summaries first.

Only return large code snippets when requested or when required by the task.

### 11.3 Token budget awareness

Every context-producing tool should accept a budget parameter.

Example:

```json
{
  "token_budget": 5000
}
```

If the result exceeds budget, return:

- selected high-priority context
- omitted items
- explanation of truncation

### 11.4 No hidden code mutation

MCP tools in MVP should be read-only unless explicitly designed otherwise.

Do not silently modify user files.

---

## 12. Security and Privacy

### 12.1 Local-only default

Never upload source code, indexes, telemetry, or memory to a remote service by default.

### 12.2 Explicit consent

Any future remote feature must require explicit user opt-in.

### 12.3 Path safety

All file operations must be constrained to the configured repository root unless explicitly allowed.

Protect against:

- path traversal
- symlink escape
- accidental home-directory indexing
- reading secrets
- indexing `.env`, key files, credentials, build outputs, and dependency folders by default

### 12.4 Default ignored paths

Ignore common heavy or sensitive paths:

```text
.git/
node_modules/
target/
dist/
build/
.cache/
.venv/
venv/
__pycache__/
.env
.env.*
*.pem
*.key
*.crt
```

Allow project-specific overrides.

---

## 13. ESG / Token good Layer

This is not part of the first kernel implementation, but the architecture should preserve the ability to measure savings.

Record local metrics:

- Context Pack token estimate
- baseline token estimate
- avoided input tokens
- avoided output tokens
- avoided file reads
- response latency
- index latency
- tool call count

Future dashboard:

```text
Token saved -> estimated API spend saved -> estimated kWh / CO2e range -> optional donation rule
```

Do not make exact carbon claims without transparent assumptions.

Use ranges and configurable coefficients.

---

## 14. Testing Strategy

### 14.1 Unit tests

Required for:

- parser extraction
- symbol normalization
- graph edge construction
- diff parsing
- overlay resolution
- Context Pack ranking
- token budget enforcement
- memory promotion rules

### 14.2 Integration tests

Create fixture repositories under:

```text
tests/fixtures/
```

Use small synthetic repos to test:

- basic indexing
- changed-file reindex
- function rename
- import update
- call graph expansion
- PR overlay
- Context Pack output

### 14.3 Golden tests

For `find_context` and `explain_pr`, use golden output tests.

The output should be deterministic for a fixed repo state.

### 14.4 Benchmarks

Add benchmarks only after correctness stabilizes.

Benchmark:

- cold index time
- warm index time
- incremental update time
- graph traversal latency
- Context Pack generation latency
- memory usage

---

## 15. MVP Acceptance Criteria

The first working MVP is acceptable when it can:

1. Start as a local MCP Server.
2. Index at least one real repository.
3. Extract file-level and symbol-level structure.
4. Store an embedded local index.
5. Re-index only changed files.
6. Generate a bounded Context Pack for a task.
7. Explain why each context item was selected.
8. Analyze a local git diff or PR-like change.
9. Return results to a coding agent through MCP.
10. Run without cloud services.

Stretch goals:

1. Basic project constitution memory.
2. Basic savings metrics.
3. Claude Code integration example.
4. OpenCode integration example.
5. VS Code wrapper.

---

## 16. Preferred Development Order

Follow this order:

```text
1. Rust workspace skeleton
2. CLI-only repo indexer
3. Tree-sitter parser for one language
4. Symbol extraction schema
5. SQLite storage
6. Graph construction
7. Simple find_context CLI
8. MCP server wrapper
9. Git diff detection
10. PR / working-tree overlay
11. explain_pr tool
12. project constitution memory
13. savings telemetry
14. host integration examples
```

Do not build IDE UI before the local kernel works.

---

## 17. Initial Language Support

Recommended first language:

```text
Rust or TypeScript
```

Choose one for MVP.

Reason:

- Tree-sitter grammar availability is good.
- Code structure is regular enough for first extraction.
- Useful for the project itself and early developer demos.

C/C++ support should be delayed unless there is a specific internal need, because semantic resolution is harder and may require compiler database integration.

---

## 18. Output Format for Context Pack

Use a structured format similar to:

```json
{
  "task": "Explain this PR",
  "budget": {
    "requested_tokens": 5000,
    "estimated_tokens": 3210
  },
  "summary": "This change modifies the parser entrypoint and affects symbol extraction.",
  "items": [
    {
      "kind": "symbol",
      "name": "parse_file",
      "file": "crates/gcl-parser/src/lib.rs",
      "range": {
        "start_line": 42,
        "end_line": 88
      },
      "reason": "Directly modified by diff and called by index_repo.",
      "score": 0.92,
      "provenance": {
        "git_ref": "working-tree",
        "file_hash": "..."
      }
    }
  ],
  "omitted": [
    {
      "file": "crates/gcl-parser/src/tests.rs",
      "reason": "Related test file, omitted due to token budget."
    }
  ]
}
```

---

## 19. Agent Behavior Rules

When an AI coding agent works on this repository, it should follow these rules:

1. Prefer small, reviewable changes.
2. Do not redesign the whole system unless asked.
3. Preserve local-first behavior.
4. Preserve deterministic output where possible.
5. Add tests with every behavior change.
6. Do not add network calls without explicit approval.
7. Do not introduce heavy dependencies without justification.
8. Avoid premature vector search.
9. Avoid premature multi-language complexity.
10. Keep the MCP surface small and stable.
11. Explain tradeoffs when changing architecture.
12. Keep resource usage visible.

---

## 20. Documentation Requirements

Every major module should document:

- responsibility
- input / output
- invariants
- error behavior
- performance considerations

Important docs to maintain:

```text
docs/architecture.md
docs/mcp-tools.md
docs/index-format.md
docs/context-pack.md
docs/memory-model.md
docs/security.md
docs/benchmarks.md
```

---

## 21. Naming Guidelines

Use clear names.

Preferred terms:

```text
ContextPack
ContextPlanner
SymbolGraph
RepoIndex
Snapshot
Overlay
MemoryFact
ProjectConstitution
ToolRun
TelemetryEvent
```

Avoid vague names like:

```text
Manager
Processor
Handler
Thing
Data
Info
```

unless scoped clearly.

---

## 22. Long-Term Direction

The long-term product should evolve in this order:

```text
Code retrieval
  -> Code graph
  -> PR-aware Context Pack
  -> Project memory
  -> Personalized memory
  -> Adaptive context policy
  -> Enterprise / OEM deployment
  -> Token savings dashboard
  -> ESG / Token good workflow
```

The key strategic moat is:

> A local, graph-native, PR-aware, memory-enabled context engine that gets smarter with every project interaction while remaining agent-agnostic.

---

## 23. Final Principle

When uncertain, optimize for:

```text
local correctness > flashy features
deterministic graph facts > vague semantic guesses
minimal sufficient context > maximum recall
observable resource usage > hidden complexity
portable MCP integration > host lock-in
```
