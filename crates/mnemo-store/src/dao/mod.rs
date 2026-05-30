//! Data-access layer for the per-project index database.
//!
//! Every DAO function borrows a `&Connection` (or `&Transaction`) and never
//! holds the handle. Per DESIGN §2.3 the graph is versioned through MVCC
//! visibility intervals: a `*_version` row is visible at snapshot `S` when
//!
//! ```text
//! visible_from <= S AND (visible_until IS NULL OR S < visible_until)
//! ```
//!
//! **Invariant** (PLAN §3 M1.3): every read takes an explicit [`SnapshotId`].
//! There is no "read latest" shortcut inside the DAO — callers resolve the
//! current snapshot via [`snapshot::latest`] and pass it down explicitly.

pub mod edge;
pub mod file;
pub mod gc;
pub mod meta;
pub mod snapshot;
pub mod symbol;
pub mod telemetry;
pub mod usefulness;
