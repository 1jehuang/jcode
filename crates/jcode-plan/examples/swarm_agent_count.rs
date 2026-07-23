//! Measure how many agents a swarm task graph would spawn.
//!
//! This drives the *real* task-DAG engine (`jcode_plan::dag`) with scripted mock
//! workers and counts the things that map to live runtime behaviour:
//!
//! * **nodes**: total nodes in the final graph, including auto-inserted gates.
//! * **dispatches**: worker turns. Under `run_plan`'s default (`prefer_spawn=true`),
//!   each dispatch is a *fresh* spawned agent, so this is the agent-spawn count.
//! * **gates**: critique/verify gates auto-inserted in deep mode.
//! * **peak concurrency**: max nodes runnable at once (the natural parallelism),
//!   which is what `run_plan`'s `concurrency_limit` (default 3) would clamp.
//!
//! Run with: `cargo run -p jcode-plan --example swarm_agent_count`
//!
//! The point is to replace hand-waving ("it spawns a lot") with reproducible
//! numbers for several representative task shapes, and to show how the deep-mode
//! gate machinery and gap injection inflate the agent count versus light mode.

use std::collections::{HashMap, HashSet};

use jcode_plan::dag::sim::deep_artifact;
use jcode_plan::dag::{
    HandoffArtifact, Mode, NodeKind, NodeSpec, TaskGraph, complete_node, dispatch, expand_node,
    fail_node, inject_from_gate, ready_nodes, seed,
};

/// What a scripted worker decides to do with a dispatched node.
#[derive(Clone)]
enum Act {
    Complete,
    Expand(Vec<NodeSpec>),
    InjectGap(Vec<NodeSpec>),
    #[allow(dead_code)]
    Fail,
}

/// A scenario is a mode, a seed, and a per-node behaviour script (by node id).
struct Scenario {
    name: &'static str,
    mode: Mode,
    seed: Vec<NodeSpec>,
    /// Returns the action for a node the *first* time it is dispatched. Composite
    /// nodes are dispatched twice (expand, then synthesis re-wake); the synthesis
    /// re-wake always just completes.
    script: HashMap<String, Act>,
}

/// Result of running a scenario through the engine.
#[derive(Default)]
struct Measured {
    nodes_final: usize,
    gates: usize,
    dispatches: usize,
    expansions: usize,
    gaps_injected: usize,
    peak_concurrency_unbounded: usize,
    steps: usize,
    stalled: bool,
}

fn spec(id: &str, kind: NodeKind) -> NodeSpec {
    NodeSpec::new(id, format!("task {id}"), kind)
}

/// Drive a scenario to completion with an *unbounded* worker pool so we can read
/// the natural peak concurrency, while still counting every dispatch (= agent).
fn measure(scn: &Scenario) -> Measured {
    let mut g = TaskGraph::new(scn.mode);
    if let Err(err) = seed(&mut g, scn.seed.clone()) {
        eprintln!("scenario seed failed to validate: {err}");
        return Measured {
            stalled: true,
            ..Measured::default()
        };
    }

    let mut script = scn.script.clone();
    let mut done_once: HashSet<String> = HashSet::new();
    let mut m = Measured::default();
    let max_steps = 10_000;

    loop {
        if g.all_terminal() {
            break;
        }
        if m.steps >= max_steps {
            m.stalled = true;
            break;
        }
        let ready: Vec<(String, NodeKind)> = ready_nodes(&g)
            .into_iter()
            .map(|n| (n.id.clone(), n.kind))
            .collect();
        if ready.is_empty() {
            m.stalled = true;
            break;
        }
        // Natural parallelism this step (unbounded pool dispatches all ready).
        m.peak_concurrency_unbounded = m.peak_concurrency_unbounded.max(ready.len());

        for (idx, (id, _kind)) in ready.into_iter().enumerate() {
            let worker = format!("w{idx}");
            if !dispatch(&mut g, &id, &worker) {
                continue;
            }
            m.dispatches += 1;

            // A node already expanded that re-wakes for synthesis just completes.
            let already = done_once.contains(&id);
            let act = if already {
                Act::Complete
            } else {
                script.remove(&id).unwrap_or(Act::Complete)
            };
            done_once.insert(id.clone());

            let step = match act {
                Act::Complete => {
                    let art = if scn.mode.requires_gates() {
                        deep_artifact(&format!("did {id}"))
                    } else {
                        HandoffArtifact::brief(format!("did {id}"))
                    };
                    complete_node(&mut g, &id, &worker, art)
                }
                Act::Expand(children) => {
                    m.expansions += 1;
                    expand_node(&mut g, &id, &worker, children).map(|_| ())
                }
                Act::InjectGap(nodes) => {
                    m.gaps_injected += nodes.len();
                    inject_from_gate(&mut g, &id, &worker, nodes).map(|_| ())
                }
                Act::Fail => fail_node(&mut g, &id, &worker),
            };
            if let Err(err) = step {
                eprintln!("scenario step on '{id}' failed: {err}");
                m.stalled = true;
                break;
            }
            m.steps += 1;
        }
    }

    m.nodes_final = g.nodes().len();
    m.gates = g.nodes().iter().filter(|n| n.is_gate).count();
    m
}

/// Scenario 1: light flat fan-out. N independent implement tasks + 1 merge.
fn light_fanout(n: usize) -> Scenario {
    let mut seed_nodes = Vec::new();
    let mut deps = Vec::new();
    for i in 0..n {
        let id = format!("t{i}");
        seed_nodes.push(spec(&id, NodeKind::Implement));
        deps.push(id);
    }
    seed_nodes.push(spec("merge", NodeKind::Synthesize).depends_on(deps));
    Scenario {
        name: "light: flat fan-out (N impl + merge)",
        mode: Mode::Light,
        seed: seed_nodes,
        script: HashMap::new(),
    }
}

/// Scenario 2: deep shallow. One root explore decomposed into K facets. Deep mode
/// adds a critique gate + a synthesis re-wake. No gaps found.
fn deep_shallow(k: usize) -> Scenario {
    let mut script = HashMap::new();
    let children: Vec<NodeSpec> = (0..k)
        .map(|i| spec(&format!("root.{i}"), NodeKind::Explore))
        .collect();
    script.insert("root".to_string(), Act::Expand(children));
    Scenario {
        name: "deep: 1 root -> K facets (gate, no gaps)",
        mode: Mode::Deep,
        seed: vec![spec("root", NodeKind::Explore)],
        script,
    }
}

/// Scenario 3: deep shallow but the critique gate finds one gap, spawning an extra
/// node and re-running the gate (the comprehensiveness loop).
fn deep_with_gap(k: usize) -> Scenario {
    let mut script = HashMap::new();
    let children: Vec<NodeSpec> = (0..k)
        .map(|i| spec(&format!("root.{i}"), NodeKind::Explore))
        .collect();
    script.insert("root".to_string(), Act::Expand(children));
    // The auto gate id is "root::gate"; first dispatch injects a gap.
    script.insert(
        "root::gate".to_string(),
        Act::InjectGap(vec![spec("root.gap", NodeKind::Explore)]),
    );
    Scenario {
        name: "deep: 1 root -> K facets, gate finds 1 gap",
        mode: Mode::Deep,
        seed: vec![spec("root", NodeKind::Explore)],
        script,
    }
}

/// Scenario 4: deep nested. Root -> K facets; one facet itself decomposes into M
/// sub-facets (a second composite + gate). Models real recursive decomposition.
fn deep_nested(k: usize, m: usize) -> Scenario {
    let mut script = HashMap::new();
    let children: Vec<NodeSpec> = (0..k)
        .map(|i| spec(&format!("root.{i}"), NodeKind::Explore))
        .collect();
    script.insert("root".to_string(), Act::Expand(children));
    let sub: Vec<NodeSpec> = (0..m)
        .map(|i| spec(&format!("root.0.{i}"), NodeKind::Explore))
        .collect();
    script.insert("root.0".to_string(), Act::Expand(sub));
    Scenario {
        name: "deep: nested (root->K, facet0->M), 2 gates",
        mode: Mode::Deep,
        seed: vec![spec("root", NodeKind::Explore)],
        script,
    }
}

/// Scenario 5: a realistic "explore then implement then verify" deep graph.
fn deep_explore_implement_verify() -> Scenario {
    let mut script = HashMap::new();
    // explore decomposes into 3 facets of investigation.
    script.insert(
        "explore".to_string(),
        Act::Expand(vec![
            spec("explore.api", NodeKind::Explore),
            spec("explore.data", NodeKind::Explore),
            spec("explore.ui", NodeKind::Explore),
        ]),
    );
    // implement decomposes into 2 code changes.
    script.insert(
        "implement".to_string(),
        Act::Expand(vec![
            spec("impl.core", NodeKind::Implement),
            spec("impl.glue", NodeKind::Implement),
        ]),
    );
    Scenario {
        name: "deep: explore(3) -> implement(2) -> verify",
        mode: Mode::Deep,
        seed: vec![
            spec("explore", NodeKind::Explore),
            spec("implement", NodeKind::Implement).depends_on(["explore"]),
            spec("verify", NodeKind::Verify).depends_on(["implement"]),
        ],
        script,
    }
}

fn print_row(scn: &Scenario, m: &Measured) {
    // Deep mode fans out to the full ready set within the configurable live-worker
    // swarm_max_concurrent_agents budget (default 32). Light mode
    // keeps a small default (4). So the effective peak parallelism is the natural
    // ready-set width clamped by the mode's default ceiling.
    let mode_ceiling = match scn.mode {
        Mode::Deep => 32,
        Mode::Light => 4,
    };
    let effective_peak = m.peak_concurrency_unbounded.min(mode_ceiling);
    println!("{}", scn.name);
    println!(
        "    mode={:<5} nodes(final)={:<3} gates={:<2} gaps_injected={:<2} expansions={}",
        match scn.mode {
            Mode::Deep => "deep",
            Mode::Light => "light",
        },
        m.nodes_final,
        m.gates,
        m.gaps_injected,
        m.expansions,
    );
    println!(
        "    dispatches(=agents spawned, fresh-per-node)={:<3} peak_parallel(natural)={:<2} peak_parallel(mode default cap)={}",
        m.dispatches, m.peak_concurrency_unbounded, effective_peak,
    );
    if m.stalled {
        println!("    !! STALLED (engine could not drive to terminal)");
    }
    println!();
}

fn main() {
    println!("=== Swarm agent-count measurement (real dag engine) ===\n");
    println!(
        "dispatches == worker turns. run_plan defaults to a fresh spawned agent per node\n\
         (prefer_spawn=true), so dispatches is the number of agents spawned. Composite\n\
         nodes are dispatched twice (decompose, then synthesis re-wake) so they cost 2.\n\
         peak_parallel(natural) is how many nodes are unblocked at once. run_plan clamps\n\
         that to a mode default: deep => agents.swarm_max_concurrent_agents (default 32,\n\
         0 = disable the configurable guard, leaving the 1000 hard cap); light => 4.\n\
         Completed workers free live-agent slots, so a run may still use more than 32\n\
         workers sequentially while never retaining more than the live budget.\n"
    );

    let scenarios = vec![
        light_fanout(4),
        light_fanout(16),
        deep_shallow(3),
        deep_shallow(6),
        deep_with_gap(3),
        deep_nested(3, 3),
        deep_explore_implement_verify(),
    ];

    for scn in &scenarios {
        let m = measure(scn);
        print_row(scn, &m);
    }

    // A compact growth table for deep shallow decomposition.
    println!("--- deep shallow: agents spawned as facet count K grows ---");
    println!("  K facets | final nodes | gates | dispatches(agents)");
    for k in [1usize, 2, 3, 4, 6, 8, 12, 16] {
        let m = measure(&deep_shallow(k));
        println!(
            "  {k:>8} | {:>11} | {:>5} | {:>17}",
            m.nodes_final, m.gates, m.dispatches
        );
    }
    println!(
        "\nFormula (deep, 1 level, no gaps): nodes = K + 2 (root + gate), \
         agents = K + 3\n  (K facet dispatches + 1 root-expand + 1 gate + 1 root-synthesis re-wake)."
    );
}
