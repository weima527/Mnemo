好。我把设计文档转成可执行的开发计划，重点是**每个里程碑的 exit criteria（怎么算做完）**和**任务之间的依赖关系**，而不是甘特图。

先做一件事：检查现有代码的真实状态，避免计划脱离实际。好，现状盘清楚了。下面是计划。

---

# Mnemo 开发计划（基于 DESIGN.md）

## 0. 计划的形状

不写时间表（你是 4-5 人小团队，估时会自欺欺人）。写**里程碑 + 入门票 + 出口标准 + 依赖关系**。每个里程碑能独立 demo，能独立 PR，能独立回滚。

```
M0 (Foundation)     ──┐
                       ├──> M1 (Vertical Slice)  ──> M2 (Multi-Tenant) ──> M3 (Productize)
M0.5 (Invariants)   ──┘
```

```
M0   地基锁死：schema + ID + 错误模型 + 目录布局        约 800 LoC
M0.5 不变量测试：3 个核心 invariant 必须红→绿          约 300 LoC
M1   垂直切片：Rust 一种语言端到端跑通 index→find      约 2500 LoC
M2   多租户 daemon：TenantManager + IPC + MCP bridge   约 2000 LoC
M3   生产化：GC + watcher + telemetry + 集成文档       约 1500 LoC
```

每个里程碑结束时必须能跑 demo，且 demo 内容固定。这是 exit gate。

---

## 1. M0 — Foundation（地基锁死）

**目标**：把"改起来会让所有人重写代码"的决策一次性做对。这个阶段不追求功能，追求**不可逆决策的正确性**。

### M0.1 错误模型重构

**入口**：当前 `CoreError` 滥用 `Io(io::Error::new(Other, ...))` 包装 git/sqlite 错误。

**任务**：

- [ ] 改 `crates/mnemo-core/src/error.rs` 为 DESIGN §9 的形状（加 `Sqlite #[from]` / `Git #[from]` / `BudgetExceeded` / `TenantUnavailable` / `Migration` / `Parse` variant）
- [ ] **加 feature flag**：`mnemo-core` 的 `Cargo.toml` 增加 `[features] git = ["dep:git2"]` 和 `sqlite = ["dep:rusqlite"]`，让 `mnemo-cli` / `mnemo-graph` 这些不需要 git/sqlite 的 crate 不被强制依赖
- [ ] 把 `mnemo-git/src/lib.rs` 里所有 `CoreError::Io(io::Error::new(...))` 改成 `CoreError::Git(e)`
- [ ] 把 `mnemo-store/src/lib.rs` 里所有 `CoreError::Internal(format!(...))` 改成对应 variant

**Exit criteria**：

```bash
cargo clippy --workspace -- -D warnings
grep -r "CoreError::Internal(format" crates/  # 必须输出 < 3 行
grep -r "io::Error::new(.*Other" crates/      # 必须 0 行
```

**估算**：~150 LoC + 改散在各处的 `?` 表达式

---

### M0.2 ID 派生系统

**入口**：当前 `FileId / SymbolId = Uuid`，每次 `Uuid::new_v4()` 生成，跨 reindex 不稳定。

**任务**：

- [ ] 新建 `crates/mnemo-core/src/ids.rs`，定义：

```rust
pub struct ProjectId([u8; 16]);
pub struct FileIdentityId([u8; 16]);
pub struct SymbolIdentityId([u8; 16]);
pub struct SymbolVersionId([u8; 16]);
pub struct SnapshotId(i64);  // 注意：INTEGER 不是 UUID

impl ProjectId {
    pub fn from_canonical_path(p: &Path) -> Result<Self> { /* blake3 + 取前 16 */ }
}
impl SymbolIdentityId {
    pub fn derive(
        project: ProjectId,
        file_path: &str,           // repo-relative, 正斜杠
        qualified_name: &str,
        kind: SymbolKind,
    ) -> Self { /* blake3 */ }
}
impl SymbolVersionId {
    pub fn derive(identity: SymbolIdentityId, content_hash: &blake3::Hash) -> Self { /* */ }
}
```

- [ ] 所有 ID 实现 `Display` (hex) / `FromStr` / `serde::{Serialize, Deserialize}` (作为 hex string)
- [ ] 所有 ID 实现 `rusqlite::ToSql` / `FromSql`（作为 BLOB 存，比 TEXT 省一半空间）
- [ ] 删除 `pub type FileId = Uuid;` 这种伪类型别名
- [ ] **`SnapshotId` 是个特例**：是 SQLite AUTOINCREMENT 的整数，不是 hash 派生。外部用 `snapshot.uuid` 字段做稳定标识

**Exit criteria**：

```rust
// 这个测试必须红→绿
#[test]
fn symbol_id_stable_across_reindex() {
    let p = ProjectId::from_canonical_path(Path::new("/tmp/x"))?;
    let id1 = SymbolIdentityId::derive(p, "src/lib.rs", "foo::bar", SymbolKind::Function);
    let id2 = SymbolIdentityId::derive(p, "src/lib.rs", "foo::bar", SymbolKind::Function);
    assert_eq!(id1, id2);
}
```

**估算**：~250 LoC

**依赖**：M0.1 完成（错误类型已 ready）

---

### M0.3 Schema 重设计

**入口**：当前 `migration_v1` 是 `repo / file / symbol / edge` 老 schema，没有 identity/version 拆分，没有 MVCC。

**任务**：

- [ ] **完全重写** `crates/mnemo-store/src/schema.rs` 的 `migration_v1`，按 DESIGN §2.3 的 SQL
- [ ] 加 `SymbolKind::to_db()` / `from_db()` 和 `EdgeKind::to_db()` / `from_db()` 到 `mnemo-core/src/types.rs`
- [ ] `open_database` 强制设置 DESIGN §2.5 的全部 pragma
- [ ] 新建 `crates/mnemo-store/src/registry.rs`，负责 `~/.mnemo/registry.db` 的初始化与 CRUD
- [ ] 新建 `crates/mnemo-store/src/paths.rs`，封装 `~/.mnemo/` 各种路径解析（含 Windows `%USERPROFILE%\.mnemo\`）

**注意点**：

- 现在还没用户在用，可以直接破坏性改 schema，不要写真正的 migration 逻辑
- 但 `schema_version` 表保留，将来 v2 升级时才有 lane
- `WITHOUT ROWID` 在 `edge_version` 上必须开
- FTS5 触发器要测一下，SQLite bundled 默认带 FTS5 但有些发行版会编译时关掉，加运行时 `SELECT fts5_version()` 探测

**Exit criteria**：

```bash
# 在干净环境跑
cargo test -p mnemo-store
# 必须包含一个测试：建库后立即查 sqlite_master，验证所有表都在
# 必须包含一个测试：在 ~/.mnemo/projects/<uuid>/ 创建 DB，断言 project 目录无新文件
```

**估算**：~400 LoC SQL + Rust

**依赖**：M0.2 完成（ID 类型 ready）

---

### M0.4 路径解析与目录布局

**入口**：DESIGN §3.4 的 `resolve_project_id`，目前完全不存在。

**任务**：

- [ ] 在 `mnemo-store/src/paths.rs` 实现：

```rust
pub fn home_mnemo() -> PathBuf;                       // ~/.mnemo/
pub fn registry_db_path() -> PathBuf;                 // ~/.mnemo/registry.db
pub fn daemon_socket_path() -> PathBuf;               // ~/.mnemo/daemon.sock
pub fn project_dir(id: ProjectId) -> PathBuf;         // ~/.mnemo/projects/<uuid>/
pub fn project_db(id: ProjectId) -> PathBuf;          // .../index.db
pub fn ensure_home_layout() -> Result<()>;            // 创建目录
pub fn resolve_project_id(input: &Path) -> Result<(ProjectId, PathBuf)>;
    // 返回 (id, canonical_path)；处理 symlink、Windows 大小写
```

- [ ] **平台差异**：Windows 用 `dirs::home_dir()`，Linux/macOS 也用同一个 crate（已有 `dirs` 4.x）。加 `dirs = "5"` 到 workspace
- [ ] 处理 `~/.mnemo` 不存在的情况：自动 mkdir -p
- [ ] **不**处理用户重命名 `~` 这种 corner case

**Exit criteria**：DESIGN §3.4 的三个测试用例全过。

**估算**：~200 LoC

---

### M0 整体出口标准（demo）

```bash
$ cd /tmp/some-rust-repo
$ mnemo index --repo-path .   # 走老的 stub index_repo 也行
# 输出大致：
# project_uuid: a3f5...
# index db at:  /home/marco/.mnemo/projects/a3f5.../index.db
# files: 0 symbols: 0 edges: 0   # 因为 parser 还是 stub，但 DB 已建好

$ ls /tmp/some-rust-repo  # 跟索引前完全一样
$ ls ~/.mnemo/projects/
# a3f5xxxxxxxxxxxx/
$ sqlite3 ~/.mnemo/projects/a3f5xxxx/index.db ".tables"
# meta snapshot file_identity file_version symbol_identity symbol_version
# edge_version symbol_fts ... context_pack memory_fact telemetry_event
```

**M0 不解决**：parser 真实工作、edge 提取、cross-file resolution、daemon、MCP。这些 M1 才碰。

---

## 2. M0.5 — Invariants Net（同步进行的安全网）

跟 M0 **并行做**。这是给整个项目兜底的测试网，写不出来的话后面会一直回归。

**任务**：

- [ ] 新建 `tests/invariants/` 目录（workspace 级别的集成测试 crate）

```toml
# tests/invariants/Cargo.toml
[package]
name = "mnemo-invariants"
edition = "2021"
[dependencies]
mnemo-core = { path = "../../crates/mnemo-core" }
mnemo-index = { path = "../../crates/mnemo-index" }
mnemo-store = { path = "../../crates/mnemo-store" }
tempfile = "3"
walkdir = "2"
```

- [ ] 写 4 个核心 invariant 测试（即使下游 stub 也要让这些跑通）：

```rust
#[test] fn zero_pollution_basic();
// 索引一个 fixture repo，前后用 walkdir 算所有文件的 (path, mtime, blake3)，
// 断言完全相等

#[test] fn symbol_id_stable_across_reindex();
// 已在 M0.2

#[test] fn project_id_resolves_through_symlinks();
// 创建 symlink 指向 fixture，attach 两次，project_id 相同

#[test] fn readonly_project_indexable();
// chmod -w fixture，attach + index 不能 panic
// Windows 下 skip（unix-only 标记）
```

- [ ] **加 fixture**：`tests/fixtures/basic_rust/` 至少要有，用脚本生成或手工放 3-5 个 `.rs` 文件

**Exit criteria**：

```bash
cargo test -p mnemo-invariants
# 4 个测试全绿
```

**估算**：~300 LoC

**关键**：这些测试现在就要写，**即使覆盖的功能还是 stub**。zero_pollution 测试现在就该过，因为 stub 不会写 project 目录。一旦后面有人不小心写了 `.mnemo/` 到 project 内，这个测试立刻红。

---

## 3. M1 — Vertical Slice（Rust 单语言端到端）

**目标**：在 Rust 一种语言上，从 `mnemo index <path>` 到 `mnemo query <symbol>` 端到端跑通。每个查询能返回真实结果。

**不做**：daemon、MCP、TypeScript、多租户、overlay。这些 M2 才做。

```
file walker → parser → resolver → store (single-tx) → in-mem graph cache
                                                          ↓
                                                  CLI query commands
```

### M1.1 Parser: 真实 tree-sitter Rust extractor

**任务**：

- [ ] `mnemo-parser/Cargo.toml` 加 `tree-sitter-rust = "0.21"` (检查最新版本兼容 tree-sitter 0.24)
- [ ] `ParseResult` 加 `edges: Vec<RawEdge>` 字段
- [ ] 新建 `mnemo-parser/src/rust.rs`：

```rust
pub fn parse_rust(file_id: FileIdentityId, source: &str) -> ParseResult {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let tree = parser.parse(source, None).unwrap();
    
    let mut visitor = RustVisitor::new(file_id, source);
    visitor.walk(tree.root_node());
    
    ParseResult {
        file_id,
        language: Language::Rust,
        symbols: visitor.symbols,
        edges: visitor.edges,
        errors: visitor.errors,
    }
}
```

- [ ] `RustVisitor` 提取的节点种类（MVP 范围）：
  - `function_item` → `SymbolKind::Function`
  - `struct_item` → `SymbolKind::Struct`
  - `enum_item` → `SymbolKind::Enum`
  - `trait_item` → `SymbolKind::Trait`
  - `impl_item` → `SymbolKind::Impl`
  - `mod_item` → `SymbolKind::Module`
  - `const_item` / `static_item` → `SymbolKind::Variable`
  - `type_item` → `SymbolKind::TypeAlias`
- [ ] `RustVisitor` 提取的边（同文件即可，先不跨文件）：
  - `call_expression` 内的 identifier → `EdgeKind::Calls`（target 是个 raw name，不是 id）
  - `use_declaration` → `EdgeKind::Imports`（target 是 path string）
  - `impl_item` 里的 type → `EdgeKind::Implements`

- [ ] **关键**：`qualified_name` 构造：从 root 向下走，遇到 `mod_item` 和 `impl_item` 都加 prefix，函数最终是 `crate::module::path::function_name` 形式。MVP 不处理 generics monomorphization，generic function 当作单个 symbol。

**Exit criteria**：

```bash
mnemo parse --file crates/mnemo-parser/src/lib.rs
# 输出 ~10 个 symbol，包括 parse_file / parse_rust / ParseResult struct 等
```

**估算**：~600 LoC（tree-sitter visitor 代码量比想象大）

---

### M1.2 Resolver: same-file 引用解析

**任务**：

- [ ] 新建 `crates/mnemo-resolve/`：

```toml
[package]
name = "mnemo-resolve"
# 依赖 mnemo-core only
```

```rust
// crates/mnemo-resolve/src/lib.rs
pub struct ResolutionContext<'a> {
    pub all_symbols: &'a HashMap<SymbolIdentityId, ResolvedSymbol>,
    pub by_name: &'a HashMap<String, Vec<SymbolIdentityId>>,
}

pub struct ResolvedSymbol {
    pub identity: SymbolIdentityId,
    pub qualified_name: String,
    pub file: FileIdentityId,
}

pub fn resolve_edges(
    raw_edges: &[RawEdge],
    ctx: &ResolutionContext,
) -> (Vec<ResolvedEdge>, Vec<UnresolvedRef>);
```

- [ ] MVP 解析策略（按优先级）：
  1. 在同一文件查同名 symbol → 命中
  2. 在 by_name 全局表查同名 → 唯一命中则取；多个命中则 `UnresolvedRef::Ambiguous`
  3. 都没有 → `UnresolvedRef::NotFound`（不写 edge，记录到 telemetry）

- [ ] **不做的**：完整 Rust trait resolution、use 语句的 alias 处理、宏展开。这些 v0.4 之后再考虑。

**Exit criteria**：

```rust
#[test]
fn same_file_call_resolved() {
    // 一个 .rs 文件，foo() 调用 bar()，bar 定义在同文件
    // 解析后应该有一条 (foo, bar, Calls) edge
}
```

**估算**：~400 LoC

---

### M1.3 Store: DAO 层

**任务**：

- [ ] 新建 `mnemo-store/src/dao/` 模块组：

```
dao/
├── mod.rs
├── snapshot.rs       insert / list / latest
├── file.rs           upsert_identity / insert_version / close_version / query_at_snapshot
├── symbol.rs         upsert_identity / insert_version / close_version / query_at_snapshot
├── edge.rs           insert_version / close_version / callers_of / callees_of
└── meta.rs           get_meta / set_meta
```

- [ ] 每个 DAO 函数签名都接受 `&Connection` 或 `&Transaction`，不持有连接
- [ ] **不变量**：所有查询函数必须带 `snapshot_id` 参数（没有"读最新"的便捷方法）
- [ ] 提供 `latest_snapshot(conn)` 工具函数，CLI 层用它，DAO 层不内嵌

**Exit criteria**：

```rust
#[test]
fn insert_and_query_at_snapshot() {
    // 1. 建 snapshot S1，insert 一个 symbol
    // 2. 建 snapshot S2，close 这个 symbol
    // 3. 查 S1 时刻该 symbol → 存在
    // 4. 查 S2 时刻该 symbol → 不存在
}
```

**估算**：~700 LoC

**依赖**：M0.3 完成

---

### M1.4 Index pipeline 真实化

**任务**：

- [ ] 重写 `mnemo-index/src/lib.rs::index_repo`：

```rust
pub fn index_repo(repo_path: &Path, force: bool) -> Result<IndexResult> {
    let (project_id, canonical) = resolve_project_id(repo_path)?;
    ensure_home_layout()?;
    
    // registry 注册
    let mut registry = Registry::open()?;
    registry.upsert_project(project_id, &canonical)?;
    
    // 打开 per-project DB
    let db_path = project_db(project_id);
    let conn = open_database(&db_path)?;
    
    if force {
        // 不删 DB，而是新建一个 'manual' snapshot 重新索引一遍
        // 但实际上 force 的语义在 MVP 阶段先简化：跟普通 index 一样
    }
    
    // 决定 snapshot source
    let source = SnapshotSource::detect(&canonical);
    
    // 创建新 snapshot
    let snap_id = create_snapshot(&conn, &source)?;
    
    // 收集变更
    let changes = match &source {
        SnapshotSource::Git(g)       => detect_git_changes(g, &conn, snap_id)?,
        SnapshotSource::FileHash(fh) => detect_file_hash_changes(fh, &conn, snap_id)?,
    };
    
    // 并行 parse 变更文件
    let parsed: Vec<ParseResult> = changes.par_iter()
        .filter_map(|c| parse_file_change(c).ok())
        .collect();
    
    // resolve (single pass)
    let resolved = resolve_all(&parsed, &conn, snap_id)?;
    
    // 单事务持久化
    apply_changes(&conn, snap_id, resolved)?;
    
    Ok(IndexResult { ... })
}
```

- [ ] `detect_file_hash_changes`：
  - walkdir 全部文件
  - 计算 blake3
  - 对比 `file_version` 表里 visible_until IS NULL 的行
  - 产出 Added / Modified / Deleted / Unchanged
- [ ] `detect_git_changes`：直接调 `mnemo-git::diff_refs(prev_snapshot.commit_sha, HEAD)`
- [ ] 加 `rayon` 到 workspace dep 用于并行 parse
- [ ] **并发限制**：file walk 串行（IO 慢但简单），parse 并行（CPU bound 大头）

**Exit criteria**：

```bash
# 第一次索引
$ mnemo index --repo-path /tmp/proj
Indexed 50 files, 800 symbols, 2300 edges in 1.2s (snapshot=1)

# 不修改任何文件，第二次索引
$ mnemo index --repo-path /tmp/proj
Indexed 0 files, 0 symbols, 0 edges in 0.05s (snapshot=2, all unchanged)
# 注意：snapshot 还是新建了，但没有 version 行被写入

# DB 大小检查
$ ls -lh ~/.mnemo/projects/*/index.db
# < 10 MB for a 50-file project
```

**估算**：~600 LoC

**依赖**：M1.1, M1.2, M1.3

---

### M1.5 Graph hydrate + CLI query

**任务**：

- [ ] 重写 `mnemo-graph/src/lib.rs`：

```rust
pub struct SymbolGraph {
    snapshot: SnapshotId,
    symbols: HashMap<SymbolIdentityId, SymbolNode>,
    
    // 倒排索引：CSR-ish
    out_edges: HashMap<SymbolIdentityId, Vec<(SymbolIdentityId, EdgeKind)>>,
    in_edges:  HashMap<SymbolIdentityId, Vec<(SymbolIdentityId, EdgeKind)>>,
    
    by_name: HashMap<String, Vec<SymbolIdentityId>>,
}

impl SymbolGraph {
    pub fn hydrate(conn: &Connection, snapshot: SnapshotId) -> Result<Self>;
    
    pub fn find_by_name(&self, name: &str) -> &[SymbolIdentityId];
    pub fn callers_of(&self, id: SymbolIdentityId) -> impl Iterator<Item = SymbolIdentityId>;
    pub fn callees_of(&self, id: SymbolIdentityId) -> impl Iterator<Item = SymbolIdentityId>;
    pub fn bfs_callers(&self, start: SymbolIdentityId, max_depth: u32) -> Vec<(SymbolIdentityId, u32)>;
}
```

- [ ] 加 CLI 子命令（在 daemon 之前先用本地 CLI 验证）：

```bash
mnemo query callers <symbol_name> [--repo-path .] [--max-depth N]
mnemo query callees <symbol_name> [--repo-path .]
mnemo query search  <pattern>          # FTS5 search
mnemo query symbol  <symbol_name>      # detailed info
```

**Exit criteria**：M1 整体出口标准下面统一说

**估算**：~500 LoC

---

### M1 整体出口标准（demo）

在 `crates/mnemo-parser/` 本身上做 dogfooding：

```bash
$ mnemo index --repo-path crates/mnemo-parser
Indexed 4 files, 18 symbols, 35 edges in 230ms (snapshot=1)

$ mnemo query callers parse_file
parse_file (crates/mnemo-parser/src/lib.rs:42)
  callers:
    - main (crates/mnemo-cli/src/main.rs:?)         # 跨 crate 暂未解析，可空
    
$ mnemo query callers parse_rust  
parse_rust (crates/mnemo-parser/src/rust.rs:?)
  callers:
    - parse_file (crates/mnemo-parser/src/lib.rs:42)

$ mnemo query search "parse"
parse_file       function   crates/mnemo-parser/src/lib.rs:42
parse_rust       function   crates/mnemo-parser/src/rust.rs:?
ParseResult      struct     crates/mnemo-parser/src/lib.rs:?
ParseError       struct     crates/mnemo-parser/src/lib.rs:?

# 修改一个文件
$ touch crates/mnemo-parser/src/lib.rs  # 真改一行代码
$ mnemo index --repo-path crates/mnemo-parser
Indexed 1 file, 0 new symbols, 0 new edges in 35ms (snapshot=2)
# 注意：1 文件被 parse 但因为 content_hash 相同，0 个新 version 行

$ sqlite3 ~/.mnemo/projects/*/index.db "SELECT count(*) FROM symbol_version;"
18  # 没有重复！
```

M1 出口的几个关键 invariant 必须验证：

- ✅ 端到端 callers/callees 查询能返回真实结果
- ✅ 同文件 calls edge 被正确建立
- ✅ 不变内容不产生新 version
- ✅ DB 大小符合预期（自我索引 < 5MB）
- ✅ project 目录依然零文件（zero_pollution 测试必须依然过）

---

## 4. M2 — Multi-Tenant Daemon

**目标**：从"CLI 一次性命令"过渡到"daemon 常驻 + MCP bridge + 多 project 切换"。这是项目从工具变产品的分水岭。

### M2.1 TenantManager

**任务**：按 DESIGN §4.3 实现：

- [ ] 新建 `crates/mnemo-tenant/`：

```toml
[package]
name = "mnemo-tenant"
[dependencies]
mnemo-core = { path = "../mnemo-core" }
mnemo-store = { path = "../mnemo-store" }
mnemo-index = { path = "../mnemo-index" }
mnemo-graph = { path = "../mnemo-graph" }
dashmap = "6"
arc-swap = "1"
parking_lot = "0.12"
tokio = { workspace = true }
```

- [ ] `ProjectContext` 持有 db pool + graph cache + overlay slot
- [ ] `TenantManager::attach / detach / get` 接口
- [ ] LRU eviction：使用 `lru` crate，维护 `max_active_projects` 上限
- [ ] **Eviction 语义严格**：释放内存，不删 DB

**Exit criteria**：

```rust
#[test]
fn lru_eviction_releases_memory_keeps_db() {
    let mgr = TenantManager::new(TenantConfig { max_active_projects: 2, .. });
    mgr.attach(&fixture("a"))?;
    mgr.attach(&fixture("b"))?;
    mgr.attach(&fixture("c"))?;  // 应该触发 a 的 eviction
    
    assert!(!mgr.is_active(a_id));
    assert!(project_db(a_id).exists());  // DB 文件还在
    
    // 重新 attach a，应该 lazy hydrate
    mgr.get(&fixture("a"))?;
}
```

**估算**：~600 LoC

---

### M2.2 IPC 协议 + daemon 进程

**任务**：

- [ ] 新建 `crates/mnemo-daemon/`：

```rust
// Unix socket 用 tokio::net::UnixListener
// Windows 用 tokio + named pipe (interprocess crate)
// 协议：length-prefixed JSON-RPC 2.0
```

- [ ] 协议格式（自定义，不上 gRPC）：

```
[4 字节 length BE][N 字节 JSON-RPC payload]
```

- [ ] 实现 method handler（暂时不接 MCP，先内部用）：

```
project.attach   { path }              → { project_uuid }
project.detach   { project_uuid }      → {}
project.list                           → [{ uuid, path, status }]
project.status   { project_uuid }      → { db_size, last_indexed, ... }
project.index    { project_uuid, force }  → { snapshot_uuid, ... }
query.callers    { project_uuid, symbol, max_depth, snapshot? }
query.callees    { ... }
query.search     { ... }
daemon.status                          → { active_projects, total_memory, ... }
daemon.shutdown                        → {}
```

- [ ] 加 `tracing` 中间件记录所有请求的 latency 和 success/fail

**Exit criteria**：

```bash
# Terminal 1
$ mnemo daemon start --foreground
[INFO] Listening on /home/marco/.mnemo/daemon.sock

# Terminal 2
$ mnemo project attach /tmp/proj-a
project_uuid: a3f5...

$ mnemo project attach /tmp/proj-b
project_uuid: b7c2...

$ mnemo daemon status
Active projects:  2 / 5 (max)
  proj-a (a3f5...) — 12 MB DB, 287 symbols
  proj-b (b7c2...) — 8 MB DB, 142 symbols

$ mnemo query callers parse_file --repo-path /tmp/proj-a
parse_file
  callers:
    - main
```

**估算**：~800 LoC

---

### M2.3 MCP Bridge

**任务**：

- [ ] 新建 `crates/mnemo-mcp/` 重新接 `rmcp`（当前是 stub）
- [ ] **mnemo-mcp 不直接持有 TenantManager**——它通过 IPC 连接到 daemon
- [ ] 实现 5 个 tool 的 MCP wrapper：把 MCP tool call → IPC method 调用
- [ ] 主二进制 `mnemo-mcp` 是 stdio MCP server，被 Claude Code spawn 起来

```
Claude Code  --stdio MCP-->  mnemo-mcp (bridge)  --IPC-->  mnemo-daemon
```

- [ ] **关键**：bridge 进程启动时如果 daemon 没在跑，**自动 spawn daemon**（detached），等 socket ready 再继续

**Exit criteria**：

```json
// .claude/mcp_config.json 或对应文件
{
  "mcpServers": {
    "mnemo": {
      "command": "mnemo-mcp",
      "args": ["--project", "/tmp/proj-a"]
    }
  }
}
```

启动 Claude Code，让它通过 MCP 调 `find_context`，能拿到真实结果。

**估算**：~500 LoC

---

### M2.4 Working Tree Overlay

**任务**：按 DESIGN §6.3 实现 `WorkingTreeOverlay`：

- [ ] 加到 `mnemo-tenant::ProjectContext` 里作为可选字段
- [ ] `find_context` 等查询接 overlay-first 路径
- [ ] **不写 DB**

**Exit criteria**：

```rust
#[test]
fn overlay_changes_visible_immediately_without_db_write() {
    let ctx = attach_project(...)?;
    let db_size_before = ctx.db_file_size();
    
    ctx.update_overlay_for_file("src/foo.rs", new_content)?;
    
    let callers = ctx.query_callers("foo_function", current=true)?;
    // callers 应该反映 new_content
    
    let db_size_after = ctx.db_file_size();
    assert_eq!(db_size_before, db_size_after);  // DB 没增长
}
```

**估算**：~400 LoC

---

### M2 整体出口标准

```
✅ daemon 常驻，能同时 attach 3+ project
✅ Claude Code 通过 MCP 跑通 find_context / trace_symbol
✅ LRU 卸载工作，内存占用稳定
✅ working tree overlay 反映即时变化
✅ daemon kill -9 后能 graceful restart（DB 状态完整）
```

---

## 5. M3 — Productize

**目标**：从"能跑"到"能用"。聚焦在体验、可观测性、清理。

不展开详细任务，按重要性排：

- [ ] **GC 后台任务**：按 DESIGN §8.3，定期清理过期 snapshot
- [ ] **File watcher**：用 `notify` crate，2s debounce，自动更新 overlay
- [ ] **`mnemo daemon status` 美化**：DESIGN §8.4 的 ASCII 进度条
- [ ] **Telemetry → symbol_usefulness 物化**：增量维护，反哺 ranking
- [ ] **Context Pack scoring 实现**：DESIGN §7.2 的真实 ranking 而不是 stub
- [ ] **TypeScript extractor**：验证 LanguageExtractor 抽象
- [ ] **集成文档 + Claude Code 一键配置脚本**
- [ ] **benchmarks**：DESIGN 里提到的 P99 < 1ms 单 symbol 点查、< 500ms find_context

**估算**：~1500 LoC，分多个独立 PR

---

## 6. 关键依赖图

```
M0.1 (errors)
   │
   ├──→ M0.2 (ids)
   │       │
   │       ├──→ M0.3 (schema)
   │       │       │
   │       │       └──→ M0.4 (paths)
   │       │              │
   │       │              ├──→ M0.5 (invariants) ────────┐
   │       │              │                              │
   │       │              ├──→ M1.1 (parser)             │
   │       │              ├──→ M1.2 (resolve) ────┐      │
   │       │              └──→ M1.3 (DAO) ────────┤      │
   │                                              │      │
   │                                              ▼      │
   │                                          M1.4 (index)
   │                                              │      │
   │                                              ▼      │
   │                                          M1.5 (graph + CLI)
   │                                              │      │
   │                                              ▼      │
   │                                          [M1 demo]──┘
   │                                              │
   │                                              ▼
   │                                          M2.1 (tenant)
   │                                              │
   │                                              ▼
   │                                          M2.2 (daemon IPC)
   │                                              │
   │                                  ┌───────────┴───────────┐
   │                                  ▼                       ▼
   │                              M2.3 (MCP bridge)     M2.4 (overlay)
   │                                  │                       │
   │                                  └───────────┬───────────┘
   │                                              ▼
   │                                          [M2 demo]
   │                                              │
   └──────────────────────────────────────────────┴──→ M3 (各项独立)
```

**并行机会**：

- M0.1 / M0.5 / M1.1 可以同时开（不同人）
- M0.5 不卡 M1 进度，M1 demo 前能合上就行
- M2.3 (MCP) 和 M2.4 (overlay) 可以并行
- M3 全部并行

---

## 7. 5 人团队分工建议（基于你的团队画像）

你说团队都是技术背景。按这份 plan，建议分工：

| 人员 | 主线 | 副线 |
|---|---|---|
| **你 (Marco)** | M0 全部 (地基) + M1.3 DAO | code review 所有 P0 PR |
| **DB 内核 #2** | M0.5 invariants + M1.4 index pipeline | M3 GC 设计 |
| **系统软件 #3** | M1.1 parser (tree-sitter) + M1.5 graph | M3 watcher |
| **系统软件 #4** | M2.1 tenant + M2.2 daemon IPC | M3 telemetry |
| **AI/ML or BD #5** | M2.3 MCP bridge + M2.4 overlay + 集成测试 | M3 文档 |

如果团队里有人没碰过 Rust，从 M0.5 / M3 文档 / M1.5 CLI 切入门槛低。

---

## 8. 风险登记

| 风险 | 影响 | 缓解 |
|---|---|---|
| tree-sitter-rust 版本与 tree-sitter 0.24 不兼容 | M1.1 卡 | M0 阶段验证一个 minimal example |
| `rmcp` crate 不成熟，API 改动 | M2.3 卡 | M2.2 实现自己的 JSON-RPC，rmcp 仅作 stdio frontend |
| Windows 路径处理 corner case | M0.4 / M2.2 卡 | M0 阶段就跑 Windows CI |
| SQLite WAL 在某些文件系统（NFS / 容器）失效 | M3 用户报障 | 启动时 pragma 探测，降级 journal_mode |
| Cross-file resolution 准确度低 | M1 demo 看起来很差 | M1 只承诺 same-file，跨文件 v0.4 改进 |
| project 路径包含 unicode 或空格 | M0.4 失败 | M0.5 加专门的 fixture 测试 |

---

## 9. Definition of Done（每个 PR 通用）

每个 PR 合并前 reviewer 检查：

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 过
- [ ] `cargo test --workspace` 过
- [ ] M0.5 的 invariant 测试**全部依然过**（特别是 zero_pollution）
- [ ] 没有新的 `CoreError::Internal(format!(...))`
- [ ] 没有新的 `unsafe` 除非 issue 编号 + 注释 + 测试
- [ ] 没有新的 `Uuid::new_v4()` 作为 logical id
- [ ] 没有写入 `<project>/` 的代码
- [ ] 公共 API 有 doc comment
- [ ] 改了 schema 就改 `LATEST_VERSION`（M0 之后）

---

## 10. 立刻能做的第一个 PR

如果你今天就想动工，最小可合并的 PR 是 **M0.1 错误模型重构**：

```
PR 范围：
  - crates/mnemo-core/src/error.rs   重写
  - crates/mnemo-core/Cargo.toml      加 feature flag
  - crates/mnemo-git/src/lib.rs       替换错误包装
  - crates/mnemo-store/src/lib.rs     替换错误包装
  
代码量：~200 行 diff
风险：低
review 时间：30 分钟
为后续解锁：M0.2 / M0.3 / M1 全部
```

这个 PR 完成后下一个就做 M0.2 (ids)，然后 M0.3 (schema)。M0 这三步走完，整个项目就有了不可逆地基。

---

## 总结

计划的核心思想：

1. **M0 是不可逆决策的集中营**：错误模型、ID 派生、Schema 一旦定了不能再改，所以必须先做
2. **M0.5 是终身陪跑的安全网**：4 个不变量测试必须从第一天就绿
3. **M1 是垂直切片**：宁可少做语言只做 Rust，也要把 index → query 端到端走通
4. **M2 才碰多租户和 MCP**：这是把项目从 demo 变产品的分水岭
5. **M3 全是并行任务**：到这一步就不卡瓶颈了

每个 milestone 都有可演示的 demo，不要让任何阶段陷入"看不到东西"。这跟你说的"3-5 年 IPO，每年要有可见进展"的节奏也一致——M0+M1 是技术 demo，M2 是产品 alpha，M3 是 public beta。

要不要现在就把 M0.1 这第一个 PR 的具体代码 patch 写出来？或者你想先讨论分工 / 风险 / 某个具体技术点？