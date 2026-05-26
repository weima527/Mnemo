//! Cross-crate invariant tests.
//!
//! This crate has no library code of its own — it exists only to host the
//! integration tests under `tests/`. Those tests assert the project-wide
//! invariants that must never regress (zero-pollution, stable identities,
//! symlink resolution, read-only safety). See `tests/invariants.rs`.
