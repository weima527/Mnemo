//! Memory layer for Mnemo.
//!
//! mnemo adopts Tulving's 4-type cognitive memory model (1972) as the
//! conceptual grouping — Semantic / Episodic / Procedural / Working — and
//! instantiates it as 5 [`MemoryKind`] variants. Using the same vocabulary as
//! widely-adopted LLM-agent memory frameworks (LangChain, Letta, Mem0) keeps
//! integration friction low for upstream agent developers.
//!
//! Physically, 4 of the 5 variants persist in the shared `memory_fact` SQLite
//! table (distinguished by the `kind` column); [`MemoryKind::Session`] is
//! pure in-memory and never written to disk — same pattern as the
//! `WorkingTreeOverlay`. See DESIGN.md §2.6.

use mnemo_core::CoreError;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Memory kinds (5 variants, Tulving-aligned)
// ---------------------------------------------------------------------------

/// The 5 mnemo memory kinds, grouped by Tulving's cognitive taxonomy via
/// [`MemoryKind::tulving_group`].
///
/// Persistence rule: every variant *except* [`MemoryKind::Session`] is stored
/// in the `memory_fact` table. Use [`MemoryKind::is_persistent`] to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// **Semantic** — stable, human-editable repo facts (build commands,
    /// architecture boundaries, invariants).
    Constitution,
    /// **Semantic** — developer or team preferences (explanation depth, review
    /// style, banned patterns). May be cross-project.
    Preference,
    /// **Episodic** — time-stamped observation events (useful Context Packs,
    /// test failures, user rejections).
    Outcome,
    /// **Procedural** — versioned how-to recipes ("how to add a new language
    /// extractor"); copy-on-write across versions.
    Skill,
    /// **Working** — current session's active context (recently touched files,
    /// recent queries, active task). **Never persisted.**
    Session,
}

/// Tulving's 4-type cognitive memory taxonomy. A coarser grouping over
/// [`MemoryKind`]; the persistent identifier remains `MemoryKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TulvingGroup {
    /// Timeless facts about the world / repo.
    Semantic,
    /// Time-stamped events from past runs.
    Episodic,
    /// Versioned procedural know-how.
    Procedural,
    /// Session-local active context.
    Working,
}

impl MemoryKind {
    /// The Tulving group this kind belongs to.
    pub fn tulving_group(self) -> TulvingGroup {
        match self {
            Self::Constitution | Self::Preference => TulvingGroup::Semantic,
            Self::Outcome => TulvingGroup::Episodic,
            Self::Skill => TulvingGroup::Procedural,
            Self::Session => TulvingGroup::Working,
        }
    }

    /// Whether this kind persists in the `memory_fact` SQLite table.
    /// Only [`MemoryKind::Session`] is in-memory; everything else returns `true`.
    pub fn is_persistent(self) -> bool {
        !matches!(self, Self::Session)
    }
}

// ---------------------------------------------------------------------------
// Memory fact (persistent kinds only)
// ---------------------------------------------------------------------------

/// A single memory fact stored in `memory_fact`. Only valid for kinds where
/// [`MemoryKind::is_persistent`] is `true`; the in-memory
/// [`MemoryKind::Session`] uses a separate session-scoped structure (TBD with
/// the v1.0 memory layer implementation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Unique id.
    pub id: String,
    /// Which repository this fact belongs to (None = global).
    pub repo_id: Option<String>,
    /// Memory kind. Must be persistent (see [`MemoryKind::is_persistent`]).
    pub kind: MemoryKind,
    /// The fact body (free-form, but should be structured JSON).
    pub fact: String,
    /// Confidence in this fact.
    pub confidence: f64,
    /// Where this fact came from (human, tool, outcome signal).
    pub provenance: String,
    /// When the fact was created.
    pub created_at: String,
    /// When the fact was last updated.
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Memory store trait (abstract over SQLite)
// ---------------------------------------------------------------------------

/// Trait for persisting and querying memory facts. The `mnemo-store` crate
/// provides the implementation. Only persistent kinds are valid arguments.
pub trait MemoryStore {
    /// Insert or update a memory fact.
    fn upsert_fact(&self, fact: &MemoryFact) -> Result<(), CoreError>;

    /// Retrieve facts of a given kind for a repository.
    fn facts_by_kind(
        &self,
        repo_id: Option<&str>,
        kind: MemoryKind,
    ) -> Result<Vec<MemoryFact>, CoreError>;

    /// Retrieve all facts for a repository.
    fn facts_for_repo(&self, repo_id: &str) -> Result<Vec<MemoryFact>, CoreError>;
}

// ---------------------------------------------------------------------------
// Memory promotion
// ---------------------------------------------------------------------------

/// Promotion state for a memory fact. Raw observations should not
/// automatically become stable memory; they must pass through promotion
/// stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
    /// A raw event, not yet evaluated.
    Raw,
    /// Promoted to candidate memory (under observation).
    Candidate,
    /// Confirmed as stable memory.
    Confirmed,
    /// Promoted to project constitution (human-verified).
    Constitution,
}

/// Evaluate whether a raw memory fact should be promoted.
///
/// Current heuristic: promote after N independent observations with consistent
/// signals. Real policy lands with the v1.0 memory layer implementation.
pub fn evaluate_promotion(
    _fact: &MemoryFact,
    _observation_count: u32,
    _consistency_score: f64,
) -> PromotionState {
    PromotionState::Raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_defaults_to_raw() {
        let fact = MemoryFact {
            id: "test-1".into(),
            repo_id: None,
            kind: MemoryKind::Outcome,
            fact: "some observation".into(),
            confidence: 0.7,
            provenance: "test".into(),
            created_at: "2025-01-01".into(),
            updated_at: "2025-01-01".into(),
        };
        assert_eq!(evaluate_promotion(&fact, 1, 0.5), PromotionState::Raw);
    }

    #[test]
    fn tulving_grouping_covers_all_kinds() {
        assert_eq!(
            MemoryKind::Constitution.tulving_group(),
            TulvingGroup::Semantic
        );
        assert_eq!(
            MemoryKind::Preference.tulving_group(),
            TulvingGroup::Semantic
        );
        assert_eq!(MemoryKind::Outcome.tulving_group(), TulvingGroup::Episodic);
        assert_eq!(MemoryKind::Skill.tulving_group(), TulvingGroup::Procedural);
        assert_eq!(MemoryKind::Session.tulving_group(), TulvingGroup::Working);
    }

    #[test]
    fn only_session_is_non_persistent() {
        for k in [
            MemoryKind::Constitution,
            MemoryKind::Preference,
            MemoryKind::Outcome,
            MemoryKind::Skill,
        ] {
            assert!(k.is_persistent(), "{k:?} should persist in memory_fact");
        }
        assert!(!MemoryKind::Session.is_persistent());
    }

    #[test]
    fn memory_kind_serde_is_snake_case() {
        let json = serde_json::to_string(&MemoryKind::Constitution).unwrap();
        assert_eq!(json, "\"constitution\"");
        let parsed: MemoryKind = serde_json::from_str("\"skill\"").unwrap();
        assert_eq!(parsed, MemoryKind::Skill);
    }
}
