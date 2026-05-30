//! Smoke performance tests for the in-memory graph hot paths.
//!
//! These compare wall-clock times against the DESIGN budget (§8 / §10):
//!   - single-symbol point lookup: P99 < 1ms
//!   - `find_context` (plan_context):    < 500ms
//!
//! For a 1000-symbol / 5000-edge synthetic graph (slightly larger than a
//! small self-indexed crate). To absorb Windows / CI scheduling jitter the
//! assertions use a 5× slack over the DESIGN budget — actual numbers are
//! typically orders of magnitude lower. A criterion-based bench harness for
//! proper micro-measurements is a follow-up (not in M3.11).
//!
//! The tests are intentionally regular `#[test]`s (not `#[ignore]`d) so DoD
//! catches order-of-magnitude regressions.

use mnemo_core::{EdgeKind, FileIdentityId, SymbolIdentityId, SymbolKind};
use mnemo_graph::{GraphNode, SymbolGraph};
use mnemo_index::context::plan_context;
use mnemo_index::query::{callees_in, callers_in};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const NUM_SYMBOLS: usize = 1000;
const NUM_EDGES: usize = 5000;

/// Build a synthetic graph with `NUM_SYMBOLS` nodes and `NUM_EDGES` `Calls`
/// edges drawn deterministically across the node range.
fn synth_graph() -> SymbolGraph {
    let mut nodes = Vec::with_capacity(NUM_SYMBOLS);
    for i in 0..NUM_SYMBOLS {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&(i as u64).to_be_bytes());
        nodes.push(GraphNode {
            identity: SymbolIdentityId::from_bytes(bytes),
            name: format!("sym{i:04}"),
            qualified_name: format!("module::sym{i:04}"),
            kind: SymbolKind::Function,
            file_id: FileIdentityId::ZERO,
            file_path: format!("src/file_{:03}.rs", i % 50),
            start_line: 1,
            end_line: 5,
            byte_len: 200,
        });
    }
    let mut edges = Vec::with_capacity(NUM_EDGES);
    for i in 0..NUM_EDGES {
        let from = i % NUM_SYMBOLS;
        let to = (i * 7 + 3) % NUM_SYMBOLS;
        if from == to {
            continue;
        }
        edges.push((nodes[from].identity, nodes[to].identity, EdgeKind::Calls));
    }
    SymbolGraph::build(nodes, &edges)
}

/// p99 of a sample slice (sorted in place).
fn p99(times: &mut [Duration]) -> Duration {
    times.sort();
    times[times.len() * 99 / 100]
}

#[test]
fn callers_in_p99_under_5ms() {
    let graph = synth_graph();
    let mut times = Vec::with_capacity(500);
    for i in 0..500 {
        let name = format!("sym{:04}", i % NUM_SYMBOLS);
        let start = Instant::now();
        let _ = callers_in(&graph, &name);
        times.push(start.elapsed());
    }
    let p99 = p99(&mut times);
    // DESIGN budget is 1ms; allow 5× slack for CI noise.
    assert!(
        p99 < Duration::from_millis(5),
        "callers_in P99 was {p99:?} (budget 1ms, slack 5ms)"
    );
}

#[test]
fn callees_in_p99_under_5ms() {
    let graph = synth_graph();
    let mut times = Vec::with_capacity(500);
    for i in 0..500 {
        let name = format!("sym{:04}", i % NUM_SYMBOLS);
        let start = Instant::now();
        let _ = callees_in(&graph, &name);
        times.push(start.elapsed());
    }
    let p99 = p99(&mut times);
    assert!(
        p99 < Duration::from_millis(5),
        "callees_in P99 was {p99:?} (budget 1ms, slack 5ms)"
    );
}

#[test]
fn plan_context_p99_under_500ms() {
    let graph = synth_graph();
    let usefulness = HashMap::new();
    let mut times = Vec::with_capacity(100);
    for i in 0..100 {
        let task = format!("sym{:04} module", i % NUM_SYMBOLS);
        let start = Instant::now();
        let _ = plan_context(&graph, &task, None, &[], 5000, &usefulness);
        times.push(start.elapsed());
    }
    let p99 = p99(&mut times);
    // DESIGN budget is 500ms; sampling already keeps median well under that.
    // Use 500ms directly — the algorithm is far faster than this on synthetic
    // 1000-symbol graphs and any regression past this is a real one.
    assert!(
        p99 < Duration::from_millis(500),
        "plan_context P99 was {p99:?} (budget 500ms)"
    );
}
