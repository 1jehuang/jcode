//! Task-level recursion budgets: parallel (wide) vs sequential (deep).
//!
//! Ported from harness2's `harness2/core/recursion.py` (Apache-2.0, Google
//! LLC). Each mode schedules `4k` rollouts + `3k` improvement stages including
//! shared base executions; invalid proposals get one retry before inheriting
//! the current harness. Parallel explores wider, sequential refines deeper.
//!
//! This module carries the executor-independent scheduling math: stage labels,
//! per-mode rollout counts, finals-only pool construction. The model calls and
//! rollouts themselves belong to the future `jcode refine` runner, not here.

use std::collections::{HashMap, HashSet};

use super::surface::Tree;

/// Recursion mode. On-disk tokens are `agg` (parallel) / `seq` (sequential),
/// matching harness2's published run layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursionMode {
    Parallel,
    Sequential,
}

impl RecursionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parallel => "parallel",
            Self::Sequential => "sequential",
        }
    }

    /// On-disk token used in `runs/` layout.
    pub fn disk_token(self) -> &'static str {
        match self {
            Self::Parallel => "agg",
            Self::Sequential => "seq",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "parallel" | "agg" => Some(Self::Parallel),
            "sequential" | "seq" => Some(Self::Sequential),
            _ => None,
        }
    }
}

/// Stage labels for one task at budget `k` (1-based).
/// Parallel: `inv1..invK`, `dom1..domK` candidates, then `agg1..aggK`
/// compositions. Sequential: per round `inv{t}`, `dom{t}`, compose `c{t}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlan {
    pub mode: RecursionMode,
    pub k: usize,
    pub candidate_labels: Vec<String>,
    pub final_labels: Vec<String>,
}

impl StagePlan {
    pub fn new(mode: RecursionMode, k: usize) -> Result<Self, String> {
        if k == 0 {
            return Err("recursion budget k must be >= 1".to_string());
        }
        if k > 16 {
            return Err(format!("recursion budget k={k} exceeds cap of 16"));
        }
        let (candidate_labels, final_labels) = match mode {
            RecursionMode::Parallel => {
                let mut cands = Vec::new();
                for i in 1..=k {
                    cands.push(format!("inv{i}"));
                    cands.push(format!("dom{i}"));
                }
                let finals: Vec<String> = (1..=k).map(|i| format!("agg{i}")).collect();
                (cands, finals)
            }
            RecursionMode::Sequential => {
                let mut cands = Vec::new();
                for t in 1..=k {
                    cands.push(format!("inv{t}"));
                    cands.push(format!("dom{t}"));
                }
                let finals: Vec<String> = (1..=k).map(|t| format!("c{t}")).collect();
                (cands, finals)
            }
        };
        Ok(Self {
            mode,
            k,
            candidate_labels,
            final_labels,
        })
    }

    /// Total rollouts scheduled: 4k (k base refs + 2k candidates + k finals).
    /// Matches harness2's published "4k rollouts and 3k improvement stages".
    pub fn rollout_count(&self) -> usize {
        4 * self.k
    }

    /// Improvement stages: 3k (2k propose + k compose).
    pub fn stage_count(&self) -> usize {
        3 * self.k
    }
}

/// One finished rollout, without any score. Scores arrive only via the judge,
/// after selection commits (harness2's `RunView` contract).
#[derive(Debug, Clone, PartialEq)]
pub struct Rollout {
    pub label: String,
    pub ok: bool,
    pub delivered: bool,
    pub cost_usd: f64,
    pub wall_s: f64,
    pub error: String,
}

impl Rollout {
    pub fn delivered_ok(label: &str) -> Self {
        Self {
            label: label.to_string(),
            ok: true,
            delivered: true,
            cost_usd: 0.0,
            wall_s: 0.0,
            error: String::new(),
        }
    }
}

/// Build the selection pool: exactly the k finals of `mode`. Reference
/// executions are NOT in the pool and there is no abstention by construction.
/// This is harness2's published convention (`selection_pool`).
pub fn selection_pool(
    mode: RecursionMode,
    k: usize,
    rollouts: &HashMap<String, Rollout>,
) -> HashMap<String, Rollout> {
    let plan = StagePlan::new(mode, k).unwrap_or(StagePlan {
        mode,
        k,
        candidate_labels: Vec::new(),
        final_labels: Vec::new(),
    });
    let wanted: HashSet<&str> = plan.final_labels.iter().map(String::as_str).collect();
    rollouts
        .iter()
        .filter(|(label, _)| wanted.contains(label.as_str()))
        .map(|(l, r)| (l.clone(), r.clone()))
        .collect()
}

/// Kept-base check: a proposal that adds, removes, and modifies nothing keeps
/// the base. KEEP_BASE is a real answer, not a failure: adaptation the
/// evidence does not support costs context and buys nothing.
pub fn kept_base(before: &Tree, after: &Tree) -> bool {
    if before.len() != after.len() {
        return false;
    }
    after
        .iter()
        .all(|(k, v)| before.get(k).is_some_and(|old| old == v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_labels_match_published_convention() {
        let p = StagePlan::new(RecursionMode::Parallel, 2).unwrap();
        assert_eq!(p.candidate_labels, vec!["inv1", "dom1", "inv2", "dom2"]);
        assert_eq!(p.final_labels, vec!["agg1", "agg2"]);
        assert_eq!(p.rollout_count(), 8);
        assert_eq!(p.stage_count(), 6);
        let s = StagePlan::new(RecursionMode::Sequential, 1).unwrap();
        assert_eq!(s.candidate_labels, vec!["inv1", "dom1"]);
        assert_eq!(s.final_labels, vec!["c1"]);
    }

    #[test]
    fn invalid_budgets_rejected() {
        assert!(StagePlan::new(RecursionMode::Parallel, 0).is_err());
        assert!(StagePlan::new(RecursionMode::Parallel, 99).is_err());
    }

    #[test]
    fn pool_is_finals_only_excludes_base() {
        let mut r = HashMap::new();
        for l in ["base1", "base2", "inv1", "dom1", "agg1"] {
            r.insert(l.to_string(), Rollout::delivered_ok(l));
        }
        let pool = selection_pool(RecursionMode::Parallel, 1, &r);
        assert_eq!(
            pool.keys().collect::<HashSet<_>>(),
            HashSet::from([&"agg1".to_string()])
        );
    }

    #[test]
    fn kept_base_detects_noop() {
        let mut a = Tree::new();
        a.insert("systemprompt.md".to_string(), "v".to_string());
        let b = a.clone();
        assert!(kept_base(&a, &b));
        let mut c = a.clone();
        c.insert("systemprompt.md".to_string(), "v2".to_string());
        assert!(!kept_base(&a, &c));
    }

    #[test]
    fn mode_tokens_roundtrip() {
        assert_eq!(RecursionMode::parse("agg"), Some(RecursionMode::Parallel));
        assert_eq!(RecursionMode::parse("seq"), Some(RecursionMode::Sequential));
        assert_eq!(RecursionMode::Parallel.disk_token(), "agg");
        assert!(RecursionMode::parse("nope").is_none());
    }
}
