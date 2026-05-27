//! Context Pack planner (DESIGN §7.2): turn a task into a scored, token-budgeted
//! set of relevant symbols, drawn from the (overlay-first) in-memory graph.
//!
//! DESIGN fixes the output schema and a 0–1000 integer score, but leaves the
//! ranking to "heuristic scoring first" (roadmap P2 #19). The heuristic here:
//! anchor on task keywords, propagate to 1-hop callers/callees, boost the
//! current file / changed files, then fill a token budget in score order.

use mnemo_core::SymbolIdentityId;
use mnemo_graph::SymbolGraph;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const SCORE_EXACT: u32 = 1000;
const SCORE_SUBSTR: u32 = 600;
const PROXIMITY_NUM: u32 = 2; // proximity = anchor * 2/5 = 0.4
const PROXIMITY_DEN: u32 = 5;
const BOOST_CURRENT_FILE: u32 = 250;
const BOOST_CHANGED: u32 = 250;
const SCORE_MAX: u32 = 1000;
const DEFAULT_TOKEN_BUDGET: u32 = 5000;
const MIN_ITEM_TOKENS: u32 = 16;

/// A scored Context Pack (DESIGN §7.2 output shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub task: String,
    pub budget: Budget,
    pub summary: String,
    pub items: Vec<ContextItem>,
    pub omitted: Vec<Omitted>,
}

/// Requested vs estimated token usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub requested: u32,
    pub estimated: u32,
}

/// 1-based inclusive line range of a definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineRange {
    pub start_line: u32,
    pub end_line: u32,
}

/// One ranked item in the Context Pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Item kind (always `"symbol"` for now).
    pub kind: String,
    /// Hex `SymbolIdentityId`.
    pub identity_id: String,
    pub name: String,
    pub qualified_name: String,
    pub file: String,
    pub range: LineRange,
    /// Why this item was included.
    pub reason: String,
    /// Relevance score, 0–1000.
    pub score: u32,
}

/// A candidate dropped to stay within the token budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Omitted {
    pub file: String,
    pub reason: String,
}

/// Split a task into identifier-ish keywords (length ≥ 3, lowercased, deduped).
fn keywords(task: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in task.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if word.len() >= 3 {
            let w = word.to_lowercase();
            if !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

/// Plan a Context Pack for `task` over `graph`.
pub fn plan_context(
    graph: &SymbolGraph,
    task: &str,
    current_file: Option<&str>,
    changed_files: &[String],
    token_budget: u32,
) -> ContextPack {
    let budget = if token_budget == 0 {
        DEFAULT_TOKEN_BUDGET
    } else {
        token_budget
    };
    let kws = keywords(task);

    // 1. Anchor scores: nodes whose name/qualified_name match a task keyword.
    let mut anchor: HashMap<SymbolIdentityId, u32> = HashMap::new();
    for kw in &kws {
        for node in graph.nodes_matching(kw) {
            let s = if node.name.to_lowercase() == *kw {
                SCORE_EXACT
            } else {
                SCORE_SUBSTR
            };
            let e = anchor.entry(node.identity).or_insert(0);
            *e = (*e).max(s);
        }
    }

    // 2. Graph proximity: 1-hop callers/callees of anchors.
    let mut proximity: HashMap<SymbolIdentityId, u32> = HashMap::new();
    for (&aid, &ascore) in &anchor {
        let prop = ascore * PROXIMITY_NUM / PROXIMITY_DEN;
        for &nid in graph.callers_of(aid).iter().chain(graph.callees_of(aid)) {
            let e = proximity.entry(nid).or_insert(0);
            *e = (*e).max(prop);
        }
    }

    // 3. Candidate set: anchors ∪ neighbors ∪ current_file/changed_files nodes.
    let in_changed = |f: &str| changed_files.iter().any(|c| c == f);
    let mut candidates: HashSet<SymbolIdentityId> = HashSet::new();
    candidates.extend(anchor.keys().copied());
    candidates.extend(proximity.keys().copied());
    for node in graph.iter_nodes() {
        if current_file == Some(node.file_path.as_str()) || in_changed(&node.file_path) {
            candidates.insert(node.identity);
        }
    }

    // 4. Score + reason each candidate.
    let mut scored: Vec<(u32, u32, ContextItem)> = Vec::new(); // (score, token_cost, item)
    for id in candidates {
        let Some(node) = graph.node(id) else {
            continue;
        };
        let anchor_score = anchor.get(&id).copied().unwrap_or(0);
        let proximity_score = proximity.get(&id).copied().unwrap_or(0);
        let base = anchor_score.max(proximity_score);
        let cur = current_file == Some(node.file_path.as_str());
        let chg = in_changed(&node.file_path);
        let score =
            (base + if cur { BOOST_CURRENT_FILE } else { 0 } + if chg { BOOST_CHANGED } else { 0 })
                .min(SCORE_MAX);
        if score == 0 {
            continue;
        }

        let mut reasons: Vec<&str> = Vec::new();
        if anchor_score == SCORE_EXACT {
            reasons.push("exact name match");
        } else if anchor_score == SCORE_SUBSTR {
            reasons.push("matches task token");
        }
        if proximity_score > 0 && anchor_score == 0 {
            reasons.push("near a task match");
        }
        if cur {
            reasons.push("current file");
        }
        if chg {
            reasons.push("diff overlap");
        }

        let token_cost = ((node.byte_len / 4) as u32).max(MIN_ITEM_TOKENS);
        scored.push((
            score,
            token_cost,
            ContextItem {
                kind: "symbol".to_string(),
                identity_id: id.to_hex(),
                name: node.name.clone(),
                qualified_name: node.qualified_name.clone(),
                file: node.file_path.clone(),
                range: LineRange {
                    start_line: node.start_line,
                    end_line: node.end_line,
                },
                reason: reasons.join(" + "),
                score,
            },
        ));
    }

    // 5. Sort by score desc, then qualified_name asc (stable, deterministic).
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.qualified_name.cmp(&b.2.qualified_name))
    });

    // 6. Fill the token budget in score order; the rest are omitted.
    let mut items = Vec::new();
    let mut omitted = Vec::new();
    let mut estimated = 0u32;
    for (_, cost, item) in scored {
        if estimated + cost <= budget {
            estimated += cost;
            items.push(item);
        } else {
            omitted.push(Omitted {
                file: item.file,
                reason: "token budget".to_string(),
            });
        }
    }

    // Anchor names for the summary (top by score).
    let mut anchor_names: Vec<(u32, String)> = anchor
        .iter()
        .filter_map(|(&id, &s)| graph.node(id).map(|n| (s, n.name.clone())))
        .collect();
    anchor_names.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let top: Vec<String> = anchor_names.into_iter().take(2).map(|(_, n)| n).collect();
    let summary = if top.is_empty() {
        format!("No anchors matched the task; {} candidate(s).", items.len())
    } else {
        format!(
            "Found {} candidate(s) anchored on {}.",
            items.len(),
            top.join(", ")
        )
    };

    ContextPack {
        task: task.to_string(),
        budget: Budget {
            requested: budget,
            estimated,
        },
        summary,
        items,
        omitted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mnemo_core::{EdgeKind, FileIdentityId, SymbolKind};
    use mnemo_graph::GraphNode;

    fn node(byte: u8, name: &str, file: &str, byte_len: usize) -> GraphNode {
        GraphNode {
            identity: SymbolIdentityId::from_bytes([byte; 16]),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_id: FileIdentityId::ZERO,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 5,
            byte_len,
        }
    }

    fn id(byte: u8) -> SymbolIdentityId {
        SymbolIdentityId::from_bytes([byte; 16])
    }

    /// foo (a.rs) calls bar (a.rs); baz (b.rs) is unrelated.
    fn fixture() -> SymbolGraph {
        SymbolGraph::build(
            vec![
                node(1, "foo", "src/a.rs", 40),
                node(2, "bar", "src/a.rs", 40),
                node(3, "baz", "src/b.rs", 40),
            ],
            &[(id(1), id(2), EdgeKind::Calls)],
        )
    }

    #[test]
    fn anchor_ranks_above_proximity() {
        let pack = plan_context(&fixture(), "foo", None, &[], 5000);
        assert_eq!(pack.items[0].name, "foo");
        assert_eq!(pack.items[0].score, SCORE_EXACT);
        // bar is a callee of foo → present, but lower.
        let bar = pack.items.iter().find(|i| i.name == "bar").unwrap();
        assert!(bar.score < pack.items[0].score && bar.score > 0);
        // baz is unrelated → not included.
        assert!(pack.items.iter().all(|i| i.name != "baz"));
        assert!(pack.budget.estimated <= pack.budget.requested);
    }

    #[test]
    fn current_file_boosts_unrelated_symbol() {
        let pack = plan_context(&fixture(), "foo", Some("src/b.rs"), &[], 5000);
        let baz = pack.items.iter().find(|i| i.name == "baz").unwrap();
        assert_eq!(baz.score, BOOST_CURRENT_FILE);
        assert!(baz.reason.contains("current file"));
    }

    #[test]
    fn token_budget_omits_low_scorers() {
        // Budget fits only one item (each costs MIN_ITEM_TOKENS = 16).
        let pack = plan_context(&fixture(), "foo", None, &[], 20);
        assert_eq!(pack.items.len(), 1);
        assert_eq!(pack.items[0].name, "foo");
        assert!(!pack.omitted.is_empty());
        assert!(pack.budget.estimated <= 20);
    }
}
