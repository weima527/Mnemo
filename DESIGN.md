# Mnemo Design Doc — MVP Engineering Guide

> **状态**：v0.1 draft，对应 codebase v0.1.0
> **目的**：指导后续开发任务，明确架构决策与实现路径
> **读者**：项目内开发人员 + AI coding agent
> **更新策略**：每个 P0/P1 milestone 完成后回写一次

这份文档不追求形式化，只解决"接下来要写什么代码、为什么这么写、不能这么写"的问题。所有跟 `AGENTS.md` 冲突的地方，以这份文档为准（AGENTS.md 是产品宪法，这份是实现宪法）。

---

## 0. TL;DR — 你只看这一节也得记住的 7 件事

1. **DB 全部放在 `~/.mnemo/projects/<uuid>/`，project 目录内零文件零污染**
2. **DB 里不存源码文本，只存 `(file_path, start_byte, end_byte)`**，需要源码现读磁盘
3. **SymbolId 是内容派生 hash，不是随机 UUID** —— 跨 reindex 稳定
4. **schema 拆 `symbol_identity` + `symbol_version`，MVCC visibility 走 `(visible_from, visible_until)` 区间**
5. **一个 daemon 多 project 多租户**，TenantManager 按 LRU 卸载内存，DB 文件不动
6. **Working tree / PR overlay 是纯内存结构，绝不写 DB**；只有显式 snapshot 才落盘
7. **Git-first，无 Git 时降级到 file-hash 伪 snapshot**，无 Git 模式下 anonymous snapshot 数量有硬上限

---

## 1. 产品定位与边界

### 1.1 是什么

Mnemo 是一个**本地常驻的多租户代码索引 daemon**。它：

- 服务于 Claude Code / Codex / Cursor / OpenCode 等 MCP-兼容的 coding agent
- 为每个 attach 的 project 维护一个独立的 SQLite 索引
- 通过 MCP 协议为 agent 提供 task-shaped 工具（不是 SQL，不是 NL2Query）
- 在 PR / working tree 变更时以 MVCC overlay 方式提供差量 context，不重建整图

### 1.2 不是什么

- **不是 SaaS**：所有数据本地，不出机器
- **不是新 LLM 或新 IDE**
- **不是 grep / ripgrep 的替代品**：mnemo 提供结构化关系，纯文本搜索仍归 grep
- **不是 LSP**：LSP 是单文件 + 语义补全；mnemo 是跨文件 + AI context 选择
- **不是 vector DB**：MVP 阶段不引入 embedding（v1.0 之后可能加，但只用于自然语言注释 / 设计文档）

### 1.3 跟 CodeGraph 的区别（差异化点）

| 维度 | CodeGraph | Mnemo |
|---|---|---|
| 部署位置 | `.codegraph/` in project | `~/.mnemo/projects/<uuid>/` |
| 进程模型 | 每 project 一个 server | 单 daemon 多租户 |
| MVCC | 无（只有 head） | snapshot + overlay |
| PR 感知 | 无 | 一等公民 |
| Tool 数量 | 8 个，含重量级 explore | 5 个，全部带 token budget |
| 源码存储 | 部分 inline | 永不存源码 |
| 实现语言 | TypeScript | Rust |

---

## 2. 核心数据模型

### 2.1 概念层次

```
Project (一个本地代码库，对应一个 ~/.mnemo/projects/<uuid>/)
  └─ Repo (一个 git repo 或 plain dir，目前一个 project = 一个 repo)
       └─ Snapshot (一个时间点的不可变图视图)
            ├─ File Version (该 snapshot 下某文件的版本)
            │    └─ Symbol Version (该 snapshot 下某符号的版本)
            │         └─ Edge Version (该 snapshot 下某关系的版本)
            └─ Overlay (内存中的 PR / working tree 差量，不落盘)
```

### 2.2 三个稳定身份 ID

| ID | 派生方式 | 何时变 |
|---|---|---|
| `ProjectId` | `blake3(canonical_path)[:16]` | 路径变才变 |
| `SymbolIdentityId` | `blake3(project_id ‖ file_path ‖ qualified_name ‖ kind)[:16]` | 改名 / 移动文件 / 改 kind 才变 |
| `SymbolVersionId` | `blake3(identity_id ‖ content_hash)[:16]` | 函数体内容变就变 |

**规则**：所有外部接口（MCP tool 返回、outcome memory 引用、context_pack_item 等）**只引用 IdentityId，不引用 VersionId**。VersionId 是物理层细节，外部不该感知。

### 2.3 SQLite schema（v1，必须落实）

> 这部分替换当前 `crates/mnemo-store/src/schema.rs` 的 `migration_v1`。

```sql
-- ============ 项目级元数据 ============

-- 一个 mnemo 实例可能管多个 project，但单个 .db 只服务一个 project。
-- 所以这里 repo 表退化为 1 行（或者干脆合到 KV meta 里）。
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- 必填 key：project_uuid, canonical_path, schema_version, created_at,
--          mnemo_version, snapshot_source ('git' | 'file_hash')

-- ============ snapshot ============

CREATE TABLE IF NOT EXISTS snapshot (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,   -- 自增整数，省空间
    uuid        TEXT NOT NULL UNIQUE,                 -- 外部稳定 ID
    kind        TEXT NOT NULL,                        -- 'commit' | 'working_tree' | 'manual' | 'anonymous'
    commit_sha  TEXT,                                 -- 当 kind='commit' 时填
    label       TEXT,                                 -- 可选用户标签
    parent_id   INTEGER REFERENCES snapshot(id),      -- 用于历史追溯，可空
    created_at  INTEGER NOT NULL                      -- unix epoch (s)
);
CREATE INDEX idx_snapshot_kind_created ON snapshot(kind, created_at);

-- 注意：working_tree / anonymous 类型的 snapshot 是临时的，
-- 由 GC 过程清理。只有 'commit' 和 'manual' 是长期保留。

-- ============ 文件 ============

CREATE TABLE IF NOT EXISTS file_identity (
    id         TEXT PRIMARY KEY,           -- blake3(project_uuid ‖ canonical_rel_path)[:16]
    path       TEXT NOT NULL UNIQUE,       -- repo-relative, 正斜杠规范化
    language   TEXT NOT NULL               -- 'rust' | 'typescript' | ...
);

CREATE TABLE IF NOT EXISTS file_version (
    id            TEXT PRIMARY KEY,         -- blake3(identity_id ‖ content_hash)[:16]
    identity_id   TEXT NOT NULL REFERENCES file_identity(id),
    content_hash  TEXT NOT NULL,            -- blake3 of file bytes (hex, 32 chars enough)
    size_bytes    INTEGER NOT NULL,
    visible_from  INTEGER NOT NULL REFERENCES snapshot(id),
    visible_until INTEGER REFERENCES snapshot(id)   -- NULL = still alive at HEAD
);
CREATE INDEX idx_file_version_visible ON file_version(identity_id, visible_from, visible_until);

-- ============ 符号 ============

CREATE TABLE IF NOT EXISTS symbol_identity (
    id              TEXT PRIMARY KEY,        -- blake3(project_uuid ‖ file_path ‖ qualified_name ‖ kind)[:16]
    file_identity_id TEXT NOT NULL REFERENCES file_identity(id),
    qualified_name  TEXT NOT NULL,           -- e.g. "mnemo_parser::parse_file"
    name            TEXT NOT NULL,           -- 短名 "parse_file"
    kind            INTEGER NOT NULL         -- 整数 enum，见 SymbolKind 映射
);
CREATE INDEX idx_symbol_identity_name ON symbol_identity(name);

CREATE TABLE IF NOT EXISTS symbol_version (
    id              TEXT PRIMARY KEY,
    identity_id     TEXT NOT NULL REFERENCES symbol_identity(id),
    -- 位置（byte range 优先，line 仅辅助）
    start_byte      INTEGER NOT NULL,
    end_byte        INTEGER NOT NULL,
    start_line      INTEGER NOT NULL,
    end_line        INTEGER NOT NULL,
    -- 内容指纹（用于 dedup：若 content_hash 不变则不开新版本）
    content_hash    TEXT NOT NULL,
    -- visibility
    visible_from    INTEGER NOT NULL REFERENCES snapshot(id),
    visible_until   INTEGER REFERENCES snapshot(id)
);
CREATE INDEX idx_symbol_version_visible ON symbol_version(identity_id, visible_from, visible_until);

-- ============ 边 ============

CREATE TABLE IF NOT EXISTS edge_version (
    from_symbol_identity TEXT NOT NULL REFERENCES symbol_identity(id),
    to_symbol_identity   TEXT NOT NULL REFERENCES symbol_identity(id),
    kind                 INTEGER NOT NULL,        -- 整数 enum
    confidence           INTEGER NOT NULL,        -- 0-100, 整数代替 REAL 省 4 字节
    visible_from         INTEGER NOT NULL REFERENCES snapshot(id),
    visible_until        INTEGER REFERENCES snapshot(id),
    PRIMARY KEY (from_symbol_identity, to_symbol_identity, kind, visible_from)
) WITHOUT ROWID;
CREATE INDEX idx_edge_to ON edge_version(to_symbol_identity, kind, visible_from, visible_until);

-- ============ 全文搜索 ============

-- FTS5 虚拟表，索引 symbol qualified_name + name
CREATE VIRTUAL TABLE IF NOT EXISTS symbol_fts USING fts5(
    qualified_name,
    name,
    content='symbol_identity',
    content_rowid='rowid'
);
-- 触发器保持同步（identity 是稳定的，FTS 跟 identity 走，不跟 version 走）
CREATE TRIGGER IF NOT EXISTS symbol_identity_ai AFTER INSERT ON symbol_identity BEGIN
    INSERT INTO symbol_fts(rowid, qualified_name, name) VALUES (new.rowid, new.qualified_name, new.name);
END;
CREATE TRIGGER IF NOT EXISTS symbol_identity_ad AFTER DELETE ON symbol_identity BEGIN
    INSERT INTO symbol_fts(symbol_fts, rowid, qualified_name, name) VALUES('delete', old.rowid, old.qualified_name, old.name);
END;

-- ============ 上下文包 & 内存事实 ============

CREATE TABLE IF NOT EXISTS context_pack (
    id                TEXT PRIMARY KEY,
    snapshot_id       INTEGER NOT NULL REFERENCES snapshot(id),
    task              TEXT NOT NULL,
    token_budget      INTEGER NOT NULL,
    estimated_tokens  INTEGER NOT NULL,
    created_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS context_pack_item (
    pack_id              TEXT NOT NULL REFERENCES context_pack(id) ON DELETE CASCADE,
    rank                 INTEGER NOT NULL,
    symbol_identity_id   TEXT REFERENCES symbol_identity(id),
    file_identity_id     TEXT REFERENCES file_identity(id),
    reason               TEXT NOT NULL,
    score                INTEGER NOT NULL,       -- 0-1000
    PRIMARY KEY (pack_id, rank)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS memory_fact (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,                  -- 见 §2.6：'constitution' | 'preference' | 'outcome' | 'skill'
                                                --（'session' 是 Working memory，纯内存，不入此表）
    promotion   TEXT NOT NULL DEFAULT 'raw',    -- 'raw' | 'candidate' | 'confirmed' | 'constitution'
    body        TEXT NOT NULL,                  -- 结构化 JSON
    confidence  INTEGER NOT NULL,               -- 0-100
    provenance  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS telemetry_event (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    event_kind  INTEGER NOT NULL,               -- 整数 enum
    payload     TEXT,                           -- JSON，但常用字段下面提到要单独抽列
    created_at  INTEGER NOT NULL
);

-- 物化的聚合视图（增量维护）
CREATE TABLE IF NOT EXISTS symbol_usefulness (
    identity_id      TEXT PRIMARY KEY REFERENCES symbol_identity(id),
    times_included   INTEGER NOT NULL DEFAULT 0,
    times_accepted   INTEGER NOT NULL DEFAULT 0,
    last_seen        INTEGER NOT NULL,
    avg_score        INTEGER NOT NULL DEFAULT 0  -- 0-1000
);
```

### 2.4 EdgeKind / SymbolKind 整数映射

> 这是物理层的事，Rust 端要写转换函数。**enum 变体只能加不能改，加变体不算 schema migration**。

```rust
// crates/mnemo-core/src/types.rs
impl SymbolKind {
    pub fn to_db(self) -> i64 {
        match self {
            Self::Function   => 1,
            Self::Struct     => 2,
            Self::Enum       => 3,
            Self::TypeAlias  => 4,
            Self::Trait      => 5,
            Self::Impl       => 6,
            Self::Module     => 7,
            Self::Variable   => 8,
            Self::Macro      => 9,
            Self::Other      => 99,
        }
    }
    pub fn from_db(v: i64) -> Self { /* ... */ }
}

impl EdgeKind {
    pub fn to_db(self) -> i64 {
        match self {
            Self::Calls       => 1,
            Self::Imports     => 2,
            Self::Contains    => 3,
            Self::Implements  => 4,
            Self::FileDepends => 5,
        }
    }
}
```

### 2.5 DB pragmas（必须开）

`open_database` 里**强制设置**：

```rust
conn.pragma_update(None, "journal_mode", "WAL")?;
conn.pragma_update(None, "synchronous", "NORMAL")?;
conn.pragma_update(None, "foreign_keys", "ON")?;
conn.pragma_update(None, "cache_size", -65536)?;     // 64 MB page cache
conn.pragma_update(None, "mmap_size", 268435456)?;   // 256 MB mmap
conn.pragma_update(None, "temp_store", "MEMORY")?;
conn.pragma_update(None, "busy_timeout", 5000)?;     // 5s 写锁等待
conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;  // GC 用得上
```

---

### 2.6 Memory taxonomy（认知科学分类 + mnemo 实例化）

> 这是 v1.0 才物化的层。**这里固定分类法**，落地时按这个 schema 写，避免 Tulving (1972) 之外再发明术语。enum 形状先锁死，schema 可后续微调。

**为什么用 Tulving 的 4 类**：LangChain / Letta / Mem0 等通用 LLM agent memory 框架已经广泛采用 Episodic / Semantic / Procedural / Working 的分组术语。mnemo 作为 agent-agnostic 工具，沿用同一套词汇可以**降低上层 agent 开发者的集成与教育成本**——他们不需要学一套 mnemo 私有的 memory 分类。

**mnemo 的 5 个 `MemoryKind` 变体，按 Tulving 分组**：

| Tulving 类 | mnemo Kind | 内容 | 物理形态 | 写入特征 |
|---|---|---|---|---|
| Semantic | `Constitution` | 仓库稳定事实（build 命令、架构边界、不变量） | `memory_fact` 表，`kind='constitution'` | 写极少；人工或人工确认后写入 |
| Semantic | `Preference` | 开发者/团队偏好（解释深度、review 风格、禁用模式） | `memory_fact` 表，`kind='preference'`；可跨项目 | 写少；跨项目可放全局 |
| Episodic | `Outcome` | 带时间戳事件（useful pack / test fail / user reject） | `memory_fact` 表，`kind='outcome'`；rolling cleanup | append-mostly；跟 `telemetry_event` 同节奏 |
| Procedural | `Skill` | how-to 流程（"添加新 extractor 的步骤"、"resolve 模糊符号策略"） | `memory_fact` 表，`kind='skill'`，`body.version` 字段；copy-on-write | 写少；版本化，可回滚 |
| Working | `Session` | 当前 session 上下文（recent touched 文件、最近 query、活跃任务） | **纯内存** `ArcSwapOption<SessionMemory>` per `ProjectContext`；session 结束即丢 | 高频读写；**绝不持久化** |

**铁律**：

1. 4 类 Tulving 中前 3 类（Semantic / Episodic / Procedural，共 4 个变体）**共享同一张 `memory_fact` 表**，靠 `kind` 列区分。SQLite 在 mnemo 的量级（每项目 10²–10⁴ 条 fact）足以支撑所有 workload，无须引入第二个存储引擎。
2. Working / `Session` **永远不落盘**，跟 `WorkingTreeOverlay`（§6.3）同一物理模式：`ArcSwapOption` slot，session 结束即释放。
3. 共享 promotion pipeline：`raw → candidate → confirmed → constitution`（详见 `mnemo-memory::PromotionState`）。
4. 外部接口（MCP tool / Context Pack）只引用 `SymbolIdentityId` / `FileIdentityId`，**不引用 memory_fact 内部 id**。
5. Memory 物化是 v1.0 目标（§11）；M0–M3 阶段 `mnemo-memory` 保持 stub，**enum 形状先锁死**，避免后续破坏性 schema 改动。

---

## 3. 文件系统布局（零污染契约）

### 3.1 用户 home 下的全部内容

```
~/.mnemo/
├── config.toml             # 全局 daemon 配置
├── daemon.sock             # Unix socket（Windows: \\.\pipe\mnemo-daemon）
├── daemon.pid
├── daemon.log              # 滚动日志
├── registry.db             # 全局 project 注册表
└── projects/
    └── <project_uuid>/
        ├── meta.toml       # project 元数据（人类可读）
        ├── index.db        # 主索引
        ├── index.db-wal    # SQLite WAL（运行时存在）
        ├── index.db-shm    # SQLite shared memory（运行时存在）
        └── gc.log          # GC 历史（可选）
```

### 3.2 project 目录下：零文件

**铁律**：mnemo 不在 `<project>/` 下创建、修改、删除任何文件。包括：

- ❌ 不写 `.mnemo/`
- ❌ 不写 `.mnemo.toml`
- ❌ 不修改 `.gitignore`
- ❌ 不创建索引缓存

**唯一允许**：读文件内容。

### 3.3 registry.db schema

```sql
CREATE TABLE project (
    uuid             TEXT PRIMARY KEY,
    canonical_path   TEXT NOT NULL UNIQUE,        -- realpath 解析后的绝对路径
    display_name     TEXT NOT NULL,
    snapshot_source  TEXT NOT NULL,                -- 'git' | 'file_hash'
    created_at       INTEGER NOT NULL,
    last_attached_at INTEGER NOT NULL,
    last_indexed_at  INTEGER,
    db_size_bytes    INTEGER,                      -- 缓存值，daemon 周期更新
    status           TEXT NOT NULL                 -- 'active' | 'archived' | 'corrupted' | 'missing'
);
CREATE INDEX idx_project_path ON project(canonical_path);
```

### 3.4 path resolution 规则

```rust
fn resolve_project_id(input_path: &Path) -> Result<ProjectId> {
    // 1. canonicalize：解析 symlink，规范化 separator，处理 ..
    let canonical = std::fs::canonicalize(input_path)
        .map_err(|e| CoreError::Io(e))?;
    
    // 2. Windows: 小写驱动器盘符
    let normalized = normalize_for_platform(&canonical);
    
    // 3. 派生 UUID
    let hash = blake3::hash(normalized.to_string_lossy().as_bytes());
    Ok(ProjectId::from_bytes(&hash.as_bytes()[..16]))
}
```

**测试用例**（必须过）：

```rust
#[test]
fn same_project_different_paths_resolve_same_id() {
    // /home/user/proj 和 /home/user/../user/proj 应该是同一个 project
}
#[test]
fn symlinked_project_resolves_to_canonical() {
    // /tmp/symlink -> /home/user/proj，两者 ProjectId 相同
}
#[test]
fn moved_project_gets_new_id_until_path_rebind() {
    // 用户把 /home/user/proj 移到 /opt/proj，老 ProjectId 失效
    // 需要 `mnemo project rebind` 命令显式迁移
}
```

---

## 4. 多租户进程模型

### 4.1 进程拓扑

```
┌──────────────────────────────────────────────────────────────┐
│                        mnemo-daemon                           │
│                    (long-running process)                     │
│                                                               │
│   IPC accept loop  ←→  Unix socket / Named pipe              │
│           │                                                   │
│           ▼                                                   │
│   ┌─────────────────┐                                         │
│   │  TenantManager  │  ← 全局 registry，LRU eviction          │
│   └────────┬────────┘                                         │
│            │                                                  │
│   ┌────────┴───────────────────────┐                          │
│   │ ProjectContext (per tenant)    │                          │
│   │  ├─ db_pool: SqlitePool         │                          │
│   │  ├─ graph_cache: ArcSwap        │                          │
│   │  ├─ overlay: Option<Overlay>    │  ← working tree state   │
│   │  ├─ git_watcher: Option<Watcher>│                          │
│   │  └─ stats: AtomicStats          │                          │
│   └────────────────────────────────┘                          │
└──────────────────────────────────────────────────────────────┘
            ▲                              ▲
            │ IPC                          │ IPC
   ┌────────┴─────────┐          ┌─────────┴────────┐
   │ mnemo-mcp-bridge │          │   mnemo-cli      │
   │ (stdio ↔ IPC)    │          │  (one-shot cmds) │
   └────────┬─────────┘          └──────────────────┘
            ▲
            │ stdio MCP
   ┌────────┴─────────┐
   │   Claude CLI     │
   └──────────────────┘
```

### 4.2 为什么要 bridge

MCP 协议是 stdio + JSON-RPC 1对1。如果 daemon 直接 serve stdio，就只能服务一个 client。所以 daemon serve IPC，多个轻量 bridge 进程把 stdio 翻译成 IPC。bridge 进程**没有状态**，可以随便 spawn 和 die。

### 4.3 TenantManager 接口（Rust sketch）

```rust
// crates/mnemo-core/src/tenant.rs (新建)
pub struct TenantManager {
    projects: DashMap<ProjectId, Arc<ProjectContext>>,
    registry: Arc<Registry>,            // 包 registry.db
    lru: Mutex<LruCache<ProjectId, ()>>,
    config: TenantConfig,
}

pub struct TenantConfig {
    pub max_active_projects: usize,        // 默认 5
    pub max_total_memory_bytes: u64,       // 默认 500 MB
    pub idle_eviction_secs: u64,           // 默认 600
    pub max_per_project_db_mb: u64,        // 默认 200
}

impl TenantManager {
    pub async fn attach(&self, path: &Path) -> Result<Arc<ProjectContext>>;
    pub async fn detach(&self, id: ProjectId) -> Result<()>;
    pub async fn get(&self, path: &Path) -> Result<Arc<ProjectContext>>;
    pub fn list_active(&self) -> Vec<ProjectSummary>;
    
    async fn ensure_capacity(&self) -> Result<()>;
    async fn evict(&self, id: ProjectId) -> Result<()>;
}
```

### 4.4 Eviction 语义

"Evict" = **释放 in-memory state，保留磁盘 DB**：

- ✅ 释放 `graph_cache`（重 hydrate 走 SQLite）
- ✅ 关闭 `db_pool` 连接
- ✅ 停止 `git_watcher`
- ❌ 不删 DB 文件
- ❌ 不删 registry 行

下次访问时 lazy hydrate。

"Detach" = evict + 标记 registry.status = 'archived'（DB 文件保留）。

"Forget" = detach + 删除 DB 文件 + 删 registry 行。**唯一会删数据的操作**。

### 4.5 并发模型

- IPC accept loop 在 tokio runtime
- 每个 client 请求 spawn 一个 task
- DB 访问通过 `deadpool-sqlite` 或自己实现的小池（每 project 一个池，4-8 个 connection）
- **写连接独占**：所有写操作通过 `Mutex<WriteConnection>` 串行化，读连接可以多个并行（WAL 模式下天然支持）
- `SymbolGraph` 内存缓存用 `ArcSwap<SymbolGraph>` —— 读时无锁，重建时整体替换

---

## 5. 索引与同步流水线

### 5.1 数据流

```
detect changed files
       │
       ▼
parse changed files (per-file, parallel)
       │
       ▼
extract symbols + raw_edges
       │
       ▼
diff against base snapshot
  ├─ unchanged → skip
  ├─ modified  → 新 symbol_version（visible_from=new_snapshot）
  │              老 symbol_version.visible_until = new_snapshot
  └─ deleted   → 老 symbol_version.visible_until = new_snapshot
       │
       ▼
cross-file resolve (name → symbol_identity_id)
       │
       ▼
persist to DB in single transaction
       │
       ▼
update in-memory graph cache (ArcSwap)
```

### 5.2 变更检测

```rust
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Unchanged,   // 用于 sanity check，正常不返回
}

pub struct DetectedChange {
    pub path: PathBuf,
    pub old_hash: Option<blake3::Hash>,
    pub new_hash: Option<blake3::Hash>,
    pub kind: ChangeKind,
}
```

两种来源：

| 来源 | 何时用 | 实现 |
|---|---|---|
| Git diff | snapshot_source = git，且有 base ref | `git2::Repository::diff_tree_to_tree` 或 `diff_tree_to_workdir_with_index` |
| File-hash walk | 无 git 或 git 状态不明 | walkdir + blake3，对比 file_version 表 |

**优化**：Git 模式下永远优先用 git diff，因为它比 walkdir 快一个量级（git 已经维护了文件 metadata）。

### 5.3 增量持久化的事务边界

```rust
async fn apply_changes(
    ctx: &ProjectContext,
    snapshot_id: SnapshotId,
    changes: Vec<FileChange>,
) -> Result<()> {
    let tx = ctx.write_conn().lock().await.transaction()?;
    
    for change in changes {
        match change.kind {
            ChangeKind::Added => {
                insert_file_version(&tx, snapshot_id, &change)?;
                insert_symbol_versions(&tx, snapshot_id, &change.symbols)?;
                insert_edge_versions(&tx, snapshot_id, &change.edges)?;
            }
            ChangeKind::Modified => {
                // 老版本"封顶"
                close_file_version(&tx, snapshot_id, change.old_file_identity)?;
                close_symbol_versions(&tx, snapshot_id, change.removed_symbols)?;
                close_edge_versions(&tx, snapshot_id, change.removed_edges)?;
                // 新版本"开顶"，但只对真的变了的 symbol
                insert_file_version(&tx, snapshot_id, &change)?;
                insert_symbol_versions(&tx, snapshot_id, &change.added_or_changed_symbols)?;
                insert_edge_versions(&tx, snapshot_id, &change.added_or_changed_edges)?;
            }
            ChangeKind::Deleted => {
                close_file_version(&tx, snapshot_id, change.old_file_identity)?;
                close_all_symbols_in_file(&tx, snapshot_id, change.old_file_identity)?;
            }
            ChangeKind::Unchanged => continue,
        }
    }
    
    tx.commit()?;
    Ok(())
}
```

**关键不变量**：

1. 一个 snapshot 的所有变更必须在**单个事务**内完成。失败回滚。
2. `visible_until = snapshot_id` 表示"在 snapshot_id 出现的时候被关闭"——查询时 `WHERE visible_until > query_snapshot OR visible_until IS NULL`。
3. `content_hash` 相同的 symbol 不开新 version，只更新底层 file_version 的归属（避免无意义版本）。

### 5.4 Watcher 策略

| 平台 | API | crate |
|---|---|---|
| Linux | inotify | `notify` |
| macOS | FSEvents | `notify` |
| Windows | ReadDirectoryChangesW | `notify` |

- 单 project 一个 watcher
- debounce: **2 秒静默窗口**（CodeGraph 用这个值，实测合理）
- 只 watch 受支持语言的文件（按扩展名过滤）
- 忽略 `DEFAULT_IGNORE_PATTERNS`

**重要**：watcher 触发的 sync 不应该立即开 snapshot。策略：

- working tree 状态变化 → 更新 in-memory `WorkingTreeOverlay`
- 用户显式 `mnemo project snapshot` → 开 'manual' snapshot 落盘
- 检测到 git commit（HEAD 移动） → 开 'commit' snapshot 落盘

这是控制 DB size 的关键招数。

---

## 6. MVCC 与 Overlay

### 6.1 Snapshot 类型

| kind | 何时创建 | 何时清理 |
|---|---|---|
| `commit` | 检测到 git HEAD 变化 | 永久保留（除非 GC 主动裁） |
| `manual` | 用户 `mnemo project snapshot <label>` | 用户显式删 |
| `working_tree` | 不落盘，仅内存 | bridge 进程关闭即消失 |
| `anonymous` | 无 git 模式下自动累积 | 保留最近 5 个，旧的 GC |

### 6.2 Visibility query 模板

```sql
-- "在 snapshot S 时刻，符号 I 的活跃版本"
SELECT * FROM symbol_version
WHERE identity_id = ?
  AND visible_from <= ?
  AND (visible_until IS NULL OR visible_until > ?)
ORDER BY visible_from DESC
LIMIT 1;
```

**所有读路径都必须经过这个模板**。不要写"读最新版本"的便捷查询——所有读都带 snapshot_id 参数。

### 6.3 WorkingTreeOverlay：内存结构

```rust
pub struct WorkingTreeOverlay {
    base_snapshot: SnapshotId,
    
    // 内存中的差量
    changed_files: HashMap<FileIdentityId, OverlayFileState>,
}

pub struct OverlayFileState {
    new_content_hash: blake3::Hash,
    new_symbols: Vec<ParsedSymbol>,
    new_edges: Vec<ParsedEdge>,
    is_deleted: bool,
}

impl WorkingTreeOverlay {
    /// Overlay-first resolve. 没有就回退到 base snapshot。
    pub fn lookup_symbol(
        &self,
        ctx: &ProjectContext,
        identity_id: SymbolIdentityId,
    ) -> Result<Option<SymbolView>> {
        let file_id = identity_id.file_identity();
        if let Some(state) = self.changed_files.get(&file_id) {
            if state.is_deleted {
                return Ok(None);
            }
            if let Some(sym) = state.new_symbols.iter().find(|s| s.identity == identity_id) {
                return Ok(Some(SymbolView::from_parsed(sym)));
            }
            // 文件被改了，但这个 symbol 在改后的文件里不存在 → 视为删除
            return Ok(None);
        }
        // 文件没改 → 回 base snapshot
        ctx.lookup_symbol_at(self.base_snapshot, identity_id)
    }
}
```

**关键**：overlay **永远不写 DB**。如果用户想把 overlay 持久化，必须显式 `snapshot --from-overlay`，那是另一个动作。

### 6.4 PR 模式

```
git checkout pr/123    ← 用户在原 repo 操作
       │
       ▼
mnemo daemon 收到 git HEAD 变化通知
       │
       ▼
为新 HEAD 创建 'commit' snapshot（如果不存在）
       │
       ▼
继续提供 overlay-on-base 的 MCP 查询
```

PR analysis 工具的实现：

```rust
async fn explain_pr(base_ref: &str, head_ref: &str) -> Result<PrAnalysis> {
    let base_snap = ensure_snapshot_for_ref(base_ref).await?;
    let head_snap = ensure_snapshot_for_ref(head_ref).await?;
    
    // 利用 visible_from / visible_until 直接查"这两个 snapshot 之间变化了什么"
    let changed_symbols = query_symbols_changed_between(base_snap, head_snap)?;
    
    // graph 遍历找 impact
    let affected = trace_callers_recursive(&changed_symbols, max_depth=2)?;
    
    // 组装 Context Pack
    build_context_pack(...)
}
```

---

## 7. MCP Tool 详细规约

5 个 tool，全部带 `token_budget`，全部返回 IdentityId 而非 VersionId。

### 7.1 `index_repo`

```json
// Input
{
  "repo_path": "/home/user/proj",
  "force": false,
  "language_filter": ["rust"]   // optional
}

// Output
{
  "project_uuid": "...",
  "snapshot_uuid": "...",
  "file_count": 1234,
  "symbol_count": 56789,
  "edge_count": 234567,
  "elapsed_ms": 12345,
  "incremental": true,
  "db_size_bytes": 87654321
}
```

`force=true` 会丢弃 in-memory cache 并重新走全量扫描，但**不会重建历史 snapshot**（仍保留所有历史 version 行）。要真清空必须用 `mnemo project forget` + `attach`。

### 7.2 `find_context`

```json
// Input
{
  "task": "Why does login fail when 2FA is enabled?",
  "repo_path": "/home/user/proj",
  "current_file": "src/auth/login.rs",   // optional
  "changed_files": ["src/auth/login.rs"], // optional
  "token_budget": 5000,
  "snapshot": null   // null = current overlay/HEAD; or specific snapshot_uuid
}

// Output
{
  "task": "...",
  "snapshot_uuid": "...",
  "budget": { "requested": 5000, "estimated": 3210 },
  "summary": "Found 8 candidates anchored on login_with_2fa and TwoFactorVerifier",
  "items": [
    {
      "kind": "symbol",
      "identity_id": "...",
      "name": "login_with_2fa",
      "file": "src/auth/login.rs",
      "range": { "start_line": 42, "end_line": 88 },
      "reason": "anchor (task token 'login') + diff overlap",
      "score": 920
    }
  ],
  "omitted": [
    { "file": "tests/auth_test.rs", "reason": "token budget" }
  ]
}
```

**实现要点**：score 在内部用 0-1000 整数，避免浮点比较。

### 7.3 `trace_symbol`

```json
// Input
{
  "symbol_name": "login_with_2fa",
  "repo_path": "...",
  "file_path": "src/auth/login.rs",   // optional 用于歧义消解
  "direction": "both",                 // "callers" | "callees" | "both"
  "max_depth": 2,
  "snapshot": null
}

// Output
{
  "matched_symbol": { "identity_id": "...", "qualified_name": "..." },
  "callers": [
    { "identity_id": "...", "name": "handle_login_request", "depth": 1, "file": "..." }
  ],
  "callees": [...],
  "ambiguous_matches": []   // 同名歧义
}
```

### 7.4 `impact_analysis`

```json
// Input
{
  "symbol": "...",       // 或 file_path
  "max_depth": 3,
  "include_tests": true
}

// Output
{
  "affected_symbols": [...],
  "affected_files": [...],
  "likely_tests": [...],   // 启发式：name contains 'test' or path matches test glob
  "risk_category": "medium",   // low | medium | high
  "explanation": "Changing X affects 7 callers across 3 modules including hot path Y."
}
```

### 7.5 `explain_pr`

```json
// Input
{
  "base_ref": "main",
  "head_ref": "feature/2fa",
  "repo_path": "...",
  "token_budget": 5000
}

// Output
{
  "base_snapshot": "...",
  "head_snapshot": "...",
  "change_summary": "Adds TwoFactorVerifier and rewires login flow.",
  "changed_symbols": [...],
  "impacted_subgraph": [...],
  "likely_tests": [...],
  "context_pack": { /* 嵌套的 find_context 结果 */ },
  "risk_notes": ["modifies critical path login_with_2fa"]
}
```

### 7.6 通用约定

- 所有 tool 返回都带 `snapshot_uuid`，方便 client 缓存与 audit
- 所有 tool 都接受 `snapshot` 入参（null = current）
- 所有 tool 都校验 token budget；超过时返回 `omitted` 字段
- 错误结构统一：`{ "error": { "code": "...", "message": "...", "details": {...} } }`

---

## 8. Size 控制：硬约束 + 实现手段

### 8.1 硬约束（写进 config）

```toml
[storage]
per_project_warn_mb = 150
per_project_max_mb = 200       # 超过拒绝新增 version，触发紧急 GC
total_max_mb = 5000

[gc]
keep_commit_snapshots = 50     # 保留最近 50 个 commit snapshot
keep_manual_snapshots = "all"  # manual 都保留
keep_anonymous_snapshots = 5
keep_days = 30
run_interval_hours = 6
incremental_vacuum = true
```

### 8.2 实现手段（按效果排序）

1. **不存源码**：DB 里只有 `(file_path, start_byte, end_byte)`。需要源码现读磁盘。
2. **content_hash 去重**：内容没变不开新 symbol_version。
3. **WITHOUT ROWID**：edge_version 强制启用。
4. **整数代替字符串**：SymbolKind / EdgeKind / confidence 全用整数。
5. **integer snapshot id**：4 字节代替 36 字节 UUID。
6. **incremental vacuum**：开启 `auto_vacuum=INCREMENTAL`，GC 后 `PRAGMA incremental_vacuum`。
7. **历史裁剪**：超出 keep_* 阈值的 snapshot 整体清理（含其 visible_from 关联的 version 行，前提是没有更早的引用）。

### 8.3 GC 算法 sketch

```rust
async fn gc_project(ctx: &ProjectContext, policy: &GcPolicy) -> Result<GcReport> {
    let now = unix_now();
    let cutoff = now - policy.keep_days * 86400;
    
    let victims: Vec<SnapshotId> = ctx.db.query(
        "SELECT id FROM snapshot 
         WHERE created_at < ?
           AND kind IN ('anonymous', 'working_tree')
         UNION
         SELECT id FROM snapshot
         WHERE kind = 'commit'
           AND id NOT IN (
             SELECT id FROM snapshot WHERE kind='commit' 
             ORDER BY created_at DESC LIMIT ?
           )",
        params![cutoff, policy.keep_commit_snapshots],
    )?;
    
    let tx = ctx.write_conn().transaction()?;
    for snap in &victims {
        // 删除"在该 snapshot 出生且已死亡的" version 行
        tx.execute(
            "DELETE FROM symbol_version 
             WHERE visible_from = ? AND visible_until IS NOT NULL",
            params![snap],
        )?;
        // 将"跨越该 snapshot 的" version 行的 visible_from rewrite 到最近的存活 snapshot
        // ... (这一步比较微妙，参考 PostgreSQL 的 freeze 思路)
    }
    // 真正删 snapshot 行
    tx.execute("DELETE FROM snapshot WHERE id IN (...)", &victims)?;
    tx.commit()?;
    
    ctx.db.execute("PRAGMA incremental_vacuum", [])?;
    
    Ok(GcReport { ... })
}
```

> **TODO**：snapshot freeze 那一步需要单独写一个 RFC 文档。MVP 阶段简化为"只裁剪没人引用的 anonymous"，commit snapshot 全保留，等性能数据再优化。

### 8.4 监控指标

`mnemo daemon status` 必须显示：

```
Active projects:  3
Total memory:     287 MB / 500 MB
Total DB size:    412 MB / 5000 MB
Per-project breakdown:
  proj-A (~/code/foo)   142 MB  ░░░░░░░░░░░  (warn @ 150)
  proj-B (~/code/bar)    87 MB  ░░░░░░       
  proj-C (~/code/baz)   183 MB  ░░░░░░░░░░░░ (over warn!)

Snapshots: 234 total (210 commit, 12 manual, 12 anonymous)
Last GC:   2 hours ago, freed 18 MB
```

---

## 9. 错误模型

替换当前 `CoreError` 设计：

```rust
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("invalid {field}: {reason}")]
    InvalidInput { field: &'static str, reason: String },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("parse {file:?}: {message}")]
    Parse { file: PathBuf, message: String },

    #[error("budget exceeded: needed {needed}, available {available}")]
    BudgetExceeded { needed: u64, available: u64 },

    #[error("tenant {0:?} unavailable: {1}")]
    TenantUnavailable(ProjectId, String),

    #[error("schema migration {from} → {to}: {reason}")]
    Migration { from: u32, to: u32, reason: String },

    #[error("serialization: {0}")]
    Serialization(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

**禁止**：把 `git2::Error` 或 `rusqlite::Error` 用 `CoreError::Internal(format!(...))` 包成字符串。永远用 `#[from]`。

---

## 10. 测试策略

### 10.1 必须有的不变量测试

```rust
// crates/mnemo-core/tests/invariants.rs

#[test]
fn zero_pollution() {
    // 索引前后 project 目录的所有文件 hash 必须完全一致
}

#[test]
fn readonly_project_works() {
    // chmod -w 的 project 也能正常索引（DB 在 home 下）
}

#[test]
fn symbol_id_stable_across_reindex() {
    // 同样的源码、reindex 两次，所有 SymbolIdentityId 必须相等
}

#[test]
fn unchanged_file_does_not_create_new_version() {
    // file content_hash 没变，不应该 insert symbol_version 新行
}

#[test]
fn mvcc_visibility_at_snapshot() {
    // 在 snapshot S 查到的 symbol 集合 = S 当时的真实集合
}

#[test]
fn overlay_first_then_base() {
    // 有 overlay 时优先返回 overlay 版本
}

#[test]
fn db_size_under_budget() {
    // 索引一个 fixture repo，DB size 不超过该 fixture 的预期上限
}
```

### 10.2 Fixture 仓库

`tests/fixtures/` 下手工维护几个小 repo：

```
basic_rust/           - 5 个文件，验证 happy path
renamed_symbol/       - 函数改名，验证 identity 变 / version 关系
import_update/        - import 改动，验证 edge 更新
deleted_file/         - 删文件，验证 visible_until
pr_overlay/           - 两个 branch，验证 PR 查询
multi_file_calls/     - 跨文件 call，验证 resolution
readonly/             - 只读权限，验证零污染
big_synthetic/        - 程序生成的 1000 文件，验证 size 上限
```

### 10.3 Golden 测试

`find_context` 和 `explain_pr` 的输出对固定 fixture 应该 deterministic：

```rust
#[test]
fn golden_explain_pr_basic() {
    let actual = run_explain_pr("tests/fixtures/pr_overlay", "main", "feature");
    let expected = read_golden("tests/golden/explain_pr_basic.json");
    assert_json_eq!(actual, expected);
}
```

Golden 文件存在 repo 里，CI 必须过。要更新先跑 `cargo test -- --ignored update_goldens`。

---

## 11. 开发优先级

### P0（v0.1，必须做对，不可逆）

**目标**：schema 与核心 ID 模型锁死，永远不用再改。

1. 重写 `crates/mnemo-store/src/schema.rs` 为本文档 §2.3
2. 实现 `CoreError` 按本文档 §9
3. 实现 `ProjectId` / `SymbolIdentityId` / `SymbolVersionId` 的 stable hash 派生
4. `crates/mnemo-core/src/types.rs` 加 `SymbolKind::to_db` / `from_db` / `EdgeKind::to_db` / `from_db`
5. 在 `~/.mnemo/` 创建目录结构 + `registry.db`
6. 写零污染 / id 稳定性 / readonly 三个不变量测试（即使其它代码是 stub，这些测试要能跑通）

### P1（v0.2，端到端走通单语言）

**目标**：在 Rust 一种语言上，从 index 到 find_context 端到端 work。

7. `mnemo-parser`：真实接 `tree-sitter-rust`，返回 `ParseResult { symbols, edges, errors }`
8. `mnemo-resolve` 新建 crate：实现 same-file calls 的 resolution（cross-file 留 stub）
9. `mnemo-store` 完整 DAO：insert/close/query symbol_version、edge_version、file_version
10. `mnemo-index::index_repo` 真实实现：file hash 增量 + 单事务持久化
11. `mnemo-graph::SymbolGraph`：hydrate from SQLite + 简单 BFS callers/callees
12. `crates/mnemo-cli`：`mnemo index <path>` 端到端跑通

### P2（v0.3，多租户 + overlay）

**目标**：daemon 跑起来，多 project 切换不冲突，PR overlay work。

13. `TenantManager` 完整实现
14. Unix socket IPC + 自定义 JSON-RPC（不上 gRPC）
15. `mnemo-mcp-bridge` 进程：stdio MCP ↔ IPC
16. `mnemo daemon` CLI 子命令族
17. `WorkingTreeOverlay` 内存实现
18. Git HEAD watcher → 自动创建 commit snapshot
19. `find_context` 真实实现（先 heuristic scoring）

### P3（v0.4，体验闭环）

**目标**：能让真人用上。

20. GC 后台任务
21. `mnemo daemon status` 可观测性
22. Outcome telemetry → symbol_usefulness 物化
23. 第二种语言（TypeScript）验证 `LanguageExtractor` trait
24. Claude Code 集成文档 + 一键 install 脚本

### 之后（v1.0）

- 第三、四种语言
- Memory layer 物化（按 §2.6 的 5-variant `MemoryKind`：Constitution / Preference / Outcome / Skill / Session；前 4 入 `memory_fact`，Session 纯内存）
- Cost-based context pack planner（Cascades 思路）
- 跨 project 查询（如果有需求）

---

## 12. 开发铁律

写代码时遵守，PR review 时检查：

1. **任何 SQL 写在 schema.rs 或专门的 query 模块，不散落到业务代码**
2. **任何对 project 目录的写操作都是 bug**（除非那个文件本来就是用户的）
3. **不要往 `CoreError::Internal(String)` 里塞下游错误**，加 variant
4. **新 enum 变体只能加在末尾**，已有 to_db 整数永不复用
5. **任何 query 必须带 snapshot_id 参数**，没有"读最新"的便捷查询
6. **DB 里不存源码文本**，需要时现读磁盘
7. **overlay 永不写 DB**，要落盘走 snapshot 命令
8. **新 P0 测试必须先红再绿**（TDD 不是教条但 invariant 测试必须 TDD）
9. **添加新依赖前确认是否能在 workspace.dependencies 复用**，不要拷贝版本号
10. **`unsafe` 永远要 issue tracking + 注释 + 测试**，目前 workspace lint 设 `deny`

---

## 13. 已知未解决问题（活的 TODO 列表）

- **跨文件 resolution 的 ambiguity**：同名函数在多个文件，启发式如何排序？v0.3 决定。
- **macro expansion**：Rust macro 展开后的 symbol 怎么挂？暂时 skip macro。
- **generics**：泛型函数的 monomorphization 是否算独立 symbol？暂时不算。
- **GC freeze 算法**：跨越多 snapshot 的 version 行如何 rewrite visible_from？P3 时再设计。
- **Windows path 大小写**：NTFS 大小写不敏感但保留，规范化策略 P0 阶段先用 `to_lowercase`，后续看是否要更精细。
- **大文件保护**：超过 1 MB 的源文件是否跳过？config 加阈值。
- **二进制文件**：用扩展名白名单，不靠 magic number 检测（够用且快）。

---

## 14. 词汇表

| 术语 | 含义 |
|---|---|
| Project | 用户视角的一个代码库，对应一个 ~/.mnemo/projects/<uuid>/ |
| Tenant | daemon 视角的一个 ProjectContext 实例 |
| Snapshot | 一个时间点的不可变图视图 |
| Overlay | working tree / PR 的内存差量，不落盘 |
| Identity | 跨版本稳定的逻辑身份 |
| Version | 某个 snapshot 下的物理具体形态 |
| Visibility | 通过 (visible_from, visible_until) 区间表达的"在哪些 snapshot 中存在" |
| Context Pack | 给 agent 的最小充分上下文 |
| Bridge | stdio MCP 到 IPC 的薄翻译进程 |
| Anchor | Context Pack 的种子节点 |
| Daemon | 长期运行的 mnemo 服务进程 |

---

*Last updated: v0.1 draft*
*Maintainer: Marco + mnemo-dev*
