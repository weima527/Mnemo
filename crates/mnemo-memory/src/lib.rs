//! Memory layer for Mnemo.
//!
//! Three tiers of memory:
//! 1. **Project Constitution** — stable repo facts (build commands, architecture boundaries).
//! 2. **User / Team Preferences** — developer preferences (explanation depth, review style).
//! 3. **Outcome Memory** — observed signals from previous runs (useful Context Packs, test failures).

use mnemo_core::CoreError;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Memory fact kinds
// ---------------------------------------------------------------------------

/// The three memory layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Stable, human-editable project facts.
    ProjectConstitution,
    /// Developer or team preferences.
    UserPreference,
    /// Observed outcome signals.
    Outcome,
}

// ---------------------------------------------------------------------------
// Memory fact
// ---------------------------------------------------------------------------

/// A single memory fact stored in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Unique id.
    pub id: String,
    /// Which repository this fact belongs to (None = global).
    pub repo_id: Option<String>,
    /// Memory tier.
    pub kind: MemoryKind,
    /// The fact body (free-form, but should be structured).
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

/// Trait for persisting and querying memory facts.
///
/// The `mnemo-store` crate provides the implementation.
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

/// Promotion state for a memory fact.
///
/// Raw observations should not automatically become stable memory.
/// They must pass through promotion stages.
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
/// Current heuristic: promote after N independent observations
/// with consistent signals.
pub fn evaluate_promotion(
    _fact: &MemoryFact,
    _observation_count: u32,
    _consistency_score: f64,
) -> PromotionState {
    // Stub — will implement promotion policy later.
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
        assert_eq!(
            evaluate_promotion(&fact, 1, 0.5),
            PromotionState::Raw
        );
    }
}
