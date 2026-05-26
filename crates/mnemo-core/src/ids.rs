//! Stable content-derived identifiers for the Mnemo code intelligence layer.
//!
//! Per DESIGN §2.2, all logical identifiers are derived from content
//! using blake3, making them stable across re-index operations.
//! This is the foundation for MVCC, outcome memory, and cross-referencing.
//!
//! Three stable ID tiers:
//!
//! ```text
//! ProjectId  = blake3(canonical_path)[:16]
//! SymbolIdentityId = blake3(project ‖ file_path ‖ qualified_name ‖ kind)[:16]
//! SymbolVersionId  = blake3(identity ‖ content_hash)[:16]
//! ```
//!
//! All IDs serialize as hex strings in JSON and as 16-byte BLOBs in SQLite.
//!
//! # Stability guarantee
//!
//! Two calls to the same `derive` method with the same inputs will always
//! produce the same ID. This is the contract that lets Context Packs,
//! outcome memory, and telemetry survive across re-index.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

// ============================================================================
// 16-byte newtype macro — reduces ~300 lines of boilerplate to ~70
// ============================================================================

macro_rules! id_newtype {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            /// Create from a 16-byte slice.
            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            /// View as a byte slice.
            pub fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            /// Full lowercase hex encoding (32 chars).
            ///
            /// Unlike `Display` (which prints only the first 8 chars for
            /// readability), this is the **lossless** canonical string form and
            /// round-trips through `FromStr`. Use it for storage keys, directory
            /// names, and anywhere identity must be preserved.
            pub fn to_hex(&self) -> String {
                hex::encode(self.0)
            }

            /// Zero ID (sentinel).
            pub const ZERO: Self = Self([0u8; 16]);
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), hex::encode(&self.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Short form: first 8 chars for readability
                write!(f, "{}", &hex::encode(&self.0)[..8])
            }
        }

        impl FromStr for $name {
            type Err = hex::FromHexError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let bytes: Vec<u8> = hex::decode(s)?;
                if bytes.len() != 16 {
                    return Err(hex::FromHexError::InvalidStringLength);
                }
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&bytes);
                Ok(Self(arr))
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&hex::encode(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

// ============================================================================
// ID types
// ============================================================================

id_newtype!(
    ProjectId,
    "Stable identity for a project directory.\n\nDerived: `blake3(canonical_absolute_path)[:16]`.\nMoving the directory changes the ID."
);

id_newtype!(
    FileIdentityId,
    "Stable identity for a source file within a project.\n\nDerived: `blake3(project_id ‖ repo_relative_path)[:16]`.\nRenaming or moving the file changes the ID."
);

id_newtype!(
    SymbolIdentityId,
    "Stable identity for a symbol across versions.\n\nDerived: `blake3(project_id ‖ file_path ‖ qualified_name ‖ kind)[:16]`.\nRenaming the symbol, changing its kind, or moving the file changes the ID."
);

id_newtype!(
    SymbolVersionId,
    "Version-specific identity for a symbol.\n\nDerived: `blake3(identity_id ‖ content_hash)[:16]`.\nThe symbol body changing creates a new VersionId, but the IdentityId stays the same."
);

// ============================================================================
// SnapshotId — a special case (SQLite auto-increment integer, not a hash)
// ============================================================================

/// A point-in-time snapshot identifier.
///
/// Unlike the other IDs, `SnapshotId` is a SQLite `AUTOINCREMENT` integer —
/// it is **not** content-derived. Use `snapshot.uuid` (the `uuid` column
/// in the `snapshot` table) when a stable external reference is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotId(pub i64);

impl SnapshotId {
    /// Sentinel for "no snapshot" / "current".
    pub const CURRENT: Self = Self(0);
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<i64> for SnapshotId {
    fn from(v: i64) -> Self {
        Self(v)
    }
}

impl From<SnapshotId> for i64 {
    fn from(s: SnapshotId) -> Self {
        s.0
    }
}

// ============================================================================
// Derivation functions
// ============================================================================

impl ProjectId {
    /// Derive the project ID from a canonical absolute path.
    ///
    /// The caller is responsible for canonicalization (symlink resolution,
    /// platform normalization). See `crates/mnemo-store/src/paths.rs` (M0.4).
    pub fn from_canonical_path(canonical: &Path) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(canonical.to_string_lossy().as_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }
}

impl FileIdentityId {
    /// Derive a stable file identity.
    ///
    /// `relative_path` must use forward slashes, e.g. `"src/lib.rs"`.
    pub fn derive(project: ProjectId, relative_path: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(project.as_bytes());
        hasher.update(b"\x00");
        hasher.update(relative_path.as_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }
}

impl SymbolIdentityId {
    /// Derive a stable symbol identity.
    ///
    /// `file_path` must use forward slashes, e.g. `"src/auth/login.rs"`.
    /// `qualified_name` is the fully-qualified name, e.g. `"crate::auth::login"`.
    /// `kind` distinguishes functions from structs with the same name.
    pub fn derive(
        project: ProjectId,
        file_path: &str,
        qualified_name: &str,
        kind: crate::types::SymbolKind,
    ) -> Self {
        let kind_byte: u8 = kind.to_db() as u8;
        let mut hasher = blake3::Hasher::new();
        hasher.update(project.as_bytes());
        hasher.update(b"\x00");
        hasher.update(file_path.as_bytes());
        hasher.update(b"\x00");
        hasher.update(qualified_name.as_bytes());
        hasher.update(b"\x00");
        hasher.update(&[kind_byte]);
        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }
}

impl SymbolVersionId {
    /// Derive a version-specific identity from a symbol identity and content hash.
    pub fn derive(identity: SymbolIdentityId, content_hash: &blake3::Hash) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(identity.as_bytes());
        hasher.update(b"\x00");
        hasher.update(content_hash.as_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }
}

// ============================================================================
// SQLite BLOB round-trip (gated behind sqlite feature)
// ============================================================================

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use super::*;
    use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Value, ValueRef};
    use rusqlite::ToSql;

    /// Implements `ToSql` + `FromSql` for a 16-byte ID, storing as BLOB.
    macro_rules! sqlite_id {
        ($name:ty) => {
            impl ToSql for $name {
                fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                    Ok(ToSqlOutput::Borrowed(ValueRef::Blob(self.as_bytes())))
                }
            }

            impl FromSql for $name {
                fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                    match value {
                        ValueRef::Blob(b) if b.len() == 16 => {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(b);
                            Ok(Self::from_bytes(arr))
                        }
                        _ => Err(FromSqlError::InvalidType),
                    }
                }
            }
        };
    }

    sqlite_id!(ProjectId);
    sqlite_id!(FileIdentityId);
    sqlite_id!(SymbolIdentityId);
    sqlite_id!(SymbolVersionId);

    // SnapshotId is stored as INTEGER.
    impl ToSql for SnapshotId {
        fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
            Ok(ToSqlOutput::Owned(Value::Integer(self.0)))
        }
    }

    impl FromSql for SnapshotId {
        fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
            value.as_i64().map(SnapshotId)
        }
    }
}

// ============================================================================
// Tests — exit criteria per PLAN M0.2
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolKind;

    #[test]
    fn symbol_id_stable_across_reindex() {
        // PLAN M0.2 exit test: same inputs → same identity
        let project = ProjectId::from_canonical_path(Path::new("/tmp/test-project"));
        let id1 = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "foo::bar",
            SymbolKind::Function,
        );
        let id2 = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "foo::bar",
            SymbolKind::Function,
        );
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_name_produces_different_identity() {
        let project = ProjectId::from_canonical_path(Path::new("/tmp/test-project"));
        let id1 = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "foo::bar",
            SymbolKind::Function,
        );
        let id2 = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "foo::baz",
            SymbolKind::Function,
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn different_kind_produces_different_identity() {
        let project = ProjectId::from_canonical_path(Path::new("/tmp/test-project"));
        let id_fn = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "MyType",
            SymbolKind::Function,
        );
        let id_st = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "MyType",
            SymbolKind::Struct,
        );
        assert_ne!(id_fn, id_st);
    }

    #[test]
    fn version_changes_with_content() {
        let project = ProjectId::from_canonical_path(Path::new("/tmp/test-project"));
        let identity = SymbolIdentityId::derive(
            project,
            "src/lib.rs",
            "foo::bar",
            SymbolKind::Function,
        );
        let hash1 = blake3::hash(b"fn foo() { 1 }");
        let hash2 = blake3::hash(b"fn foo() { 2 }");

        let v1 = SymbolVersionId::derive(identity, &hash1);
        let v2 = SymbolVersionId::derive(identity, &hash2);
        assert_ne!(v1, v2);
    }

    #[test]
    fn project_id_derived_from_path() {
        let id1 = ProjectId::from_canonical_path(Path::new("/home/user/proj"));
        let id2 = ProjectId::from_canonical_path(Path::new("/home/user/proj"));
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_project_paths_yield_different_ids() {
        let id1 = ProjectId::from_canonical_path(Path::new("/home/user/proj-a"));
        let id2 = ProjectId::from_canonical_path(Path::new("/home/user/proj-b"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn id_display_is_8_char_hex() {
        let id = ProjectId::from_bytes([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(id.to_string(), "deadbeef");
    }

    #[test]
    fn to_hex_is_full_length_and_roundtrips() {
        let id = SymbolIdentityId::from_bytes(*b"abcdefgh12345678");
        let hex = id.to_hex();
        assert_eq!(hex.len(), 32, "to_hex must be the full 32-char form");
        assert_ne!(hex, id.to_string(), "to_hex must differ from short Display");
        let parsed: SymbolIdentityId = hex.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn id_roundtrip_hex_string() {
        let orig = ProjectId::from_bytes(*b"abcdefgh12345678"); // 16 bytes
        let hex_str = hex::encode(orig.as_bytes()); // full 32-char hex
        let parsed: ProjectId = hex_str.parse().unwrap();
        assert_eq!(orig, parsed);
    }

    #[test]
    fn id_roundtrip_json() {
        let id = SymbolIdentityId::from_bytes(*b"abcdefgh12345678");
        let json = serde_json::to_string(&id).unwrap();
        let parsed: SymbolIdentityId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn snapshot_id_display() {
        let snap = SnapshotId(42);
        assert_eq!(snap.to_string(), "42");
    }

    #[test]
    fn id_zero_is_sentinel() {
        assert_eq!(ProjectId::ZERO.as_bytes(), &[0u8; 16]);
    }
}
