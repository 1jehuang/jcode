//! Anonymized candidate selection with safe fallbacks.
//!
//! Ported from harness2's `harness2/core/selection.py` (Apache-2.0, Google
//! LLC). The selector sees anonymized outputs (A..Z, AA..) via a seeded
//! shuffle that is stable across reruns, so there is no credit for picking a
//! more elaborate candidate and no "final candidate to defer to". Picking an
//! output from an unmodified harness is a completely valid outcome.
//!
//! Non-answers (failed run, no deliverable, empty/missing artifact) are
//! dropped before anonymising, each drop recorded as `label (cause)`.
//! A selector failure
//! falls back to a delivered base, else the first final; it never raises.

use std::collections::HashMap;

#[cfg(test)]
use std::collections::HashSet;

use super::recursion::Rollout;

/// Seed derived from task/k/mode so the shuffle is stable across reruns.
/// harness2 uses sha1(task|k|mode); FNV-1a keeps this dependency-free with the
/// same stability property.
pub fn shuffle_seed(task_id: &str, k: usize, mode: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{task_id}|{k}|{mode}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Anon id: A..Z, AA..AZ, BA... Always accepted by the choice parser.
pub fn anon_id(mut index: usize) -> String {
    let mut out = String::new();
    index += 1;
    while index > 0 {
        let (q, r) = ((index - 1) / 26, (index - 1) % 26);
        out.insert(0, (b'A' + r as u8) as char);
        index = q;
    }
    out
}

/// anon id -> label, by canonical sort then seeded shuffle.
pub fn assign_ids(
    labels: &[String],
    task_id: &str,
    k: usize,
    mode: &str,
) -> HashMap<String, String> {
    let mut ordered: Vec<&String> = labels.iter().collect();
    ordered.sort();
    // Deterministic Fisher-Yates with SplitMix64 over the seed.
    let mut state = shuffle_seed(task_id, k, mode);
    let mut next_u64 = move || {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    };
    for i in (1..ordered.len()).rev() {
        let j = (next_u64() % (i as u64 + 1)) as usize;
        ordered.swap(i, j);
    }
    ordered
        .into_iter()
        .enumerate()
        .map(|(i, lbl)| (anon_id(i), lbl.clone()))
        .collect()
}

/// Drop non-answers before anonymising; record each drop with its cause.
/// Mirrors harness2's `_render_pool`.
pub fn render_pool(
    candidates: &HashMap<String, Rollout>,
    artifacts: &HashMap<String, String>,
) -> (
    HashMap<String, Rollout>,
    HashMap<String, String>,
    Vec<String>,
) {
    let mut pool = HashMap::new();
    let mut texts = HashMap::new();
    let mut dropped = Vec::new();
    let mut labels: Vec<&String> = candidates.keys().collect();
    labels.sort();
    for lbl in labels {
        let rv = &candidates[lbl];
        if !(rv.ok && rv.delivered) {
            let why = if !rv.ok {
                if rv.error.trim().is_empty() {
                    "run failed"
                } else {
                    rv.error.trim()
                }
            } else {
                "no deliverable"
            };
            // Truncate long errors like the source's [:200] cap.
            let why: String = why.chars().take(200).collect();
            dropped.push(format!("{lbl} ({why})"));
            continue;
        }
        match artifacts.get(lbl) {
            Some(t) if !t.trim().is_empty() => {
                pool.insert(lbl.clone(), rv.clone());
                texts.insert(lbl.clone(), t.clone());
            }
            Some(_) => dropped.push(format!("{lbl} (empty artifact)")),
            None => dropped.push(format!("{lbl} (missing artifact)")),
        }
    }
    dropped.sort();
    (pool, texts, dropped)
}

/// Outcome of parsing one selector reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorOutcome {
    Chosen {
        anon: String,
        label: String,
    },
    /// No delivered base existed for a neutral fallback.
    NoFallback {
        error: String,
    },
    Fallback {
        label: String,
        error: String,
    },
}

fn is_base(label: &str) -> bool {
    label.starts_with("base")
}

/// Resolve a selector reply (`choice` block naming one anon id) against the
/// anonymized pool. On failure: delivered base first, else first final for
/// finals pools. Never raises.
pub fn resolve_choice(
    reply_choice: Option<&str>,
    id_map: &HashMap<String, String>,
    pool: &HashMap<String, Rollout>,
    pool_kind: &str,
) -> SelectorOutcome {
    if let Some(choice) = reply_choice
        && let Some(label) = id_map.get(choice)
        && pool.contains_key(label)
    {
        return SelectorOutcome::Chosen {
            anon: choice.to_string(),
            label: label.clone(),
        };
    }
    let err = match reply_choice {
        None => "no choice block in the reply".to_string(),
        Some(c) => format!("chose {c:?}, which is not one of the candidates"),
    };
    let mut bases: Vec<&String> = pool.keys().filter(|l| is_base(l.as_str())).collect();
    bases.sort();
    if let Some(first) = bases.first() {
        return SelectorOutcome::Fallback {
            label: (*first).clone(),
            error: err,
        };
    }
    if pool_kind == "finals" {
        let mut finals: Vec<&String> = pool.keys().collect();
        finals.sort();
        if let Some(first) = finals.first() {
            return SelectorOutcome::Fallback {
                label: (*first).clone(),
                error: err,
            };
        }
    }
    SelectorOutcome::NoFallback { error: err }
}

/// Deliver the single k=1 final without a selector session. The same
/// non-answer drop rules apply; an empty pool is an error with no base
/// fallback (a finals-only pool has no base by construction). The chosen id
/// is always `A`, mirroring the source's `trivial_selection`.
pub fn trivial_selection(
    candidates: &HashMap<String, Rollout>,
    artifacts: &HashMap<String, String>,
) -> Result<(String, String), String> {
    if candidates.len() > 1 {
        let mut labels: Vec<&String> = candidates.keys().collect();
        labels.sort();
        return Err(format!(
            "trivial_selection expects at most one candidate (the k=1 final), got {labels:?}"
        ));
    }
    let (pool, _, dropped) = render_pool(candidates, artifacts);
    if pool.is_empty() {
        return Err(format!(
            "no candidate produced an artifact (finals-only pool has no base fallback; dropped: {dropped:?})"
        ));
    }
    let label = pool.keys().next().cloned().unwrap_or_default();
    Ok(("A".to_string(), label))
}

/// Supported lesson distilled after adoption. All three fields required;
/// anything less is not stored (harness2's `has_experience`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Experience {
    pub cue: String,
    pub lesson: String,
    pub evidence: String,
    pub confidence: Option<String>,
}

impl Experience {
    pub fn new(cue: &str, lesson: &str, evidence: &str) -> Option<Self> {
        if cue.trim().is_empty() || lesson.trim().is_empty() || evidence.trim().is_empty() {
            return None;
        }
        Some(Self {
            cue: cue.trim().to_string(),
            lesson: lesson.trim().to_string(),
            evidence: evidence.trim().to_string(),
            confidence: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anon_ids_count_like_spreadsheet() {
        assert_eq!(anon_id(0), "A");
        assert_eq!(anon_id(25), "Z");
        assert_eq!(anon_id(26), "AA");
        assert_eq!(anon_id(27), "AB");
    }

    #[test]
    fn assign_is_stable_and_covers_all() {
        let labels: Vec<String> = ["agg1", "agg2", "agg3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = assign_ids(&labels, "task/x", 2, "agg");
        let b = assign_ids(&labels, "task/x", 2, "agg");
        assert_eq!(a, b);
        let got: HashSet<&String> = a.values().collect();
        let want: HashSet<&String> = labels.iter().collect();
        assert_eq!(got, want);
        // Different task -> (almost surely) different shuffle; at minimum valid.
        let c = assign_ids(&labels, "task/y", 2, "agg");
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn pool_drops_non_answers_with_causes() {
        let mut cands = HashMap::new();
        cands.insert("agg1".to_string(), Rollout::delivered_ok("agg1"));
        cands.insert(
            "agg2".to_string(),
            Rollout {
                label: "agg2".to_string(),
                ok: false,
                delivered: false,
                cost_usd: 0.0,
                wall_s: 0.0,
                error: "crashed".to_string(),
            },
        );
        let mut arts = HashMap::new();
        arts.insert("agg1".to_string(), "report body".to_string());
        let (pool, texts, dropped) = render_pool(&cands, &arts);
        assert!(pool.contains_key("agg1"));
        // Each drop carries its cause, mirroring `_render_pool`.
        assert_eq!(dropped, vec!["agg2 (crashed)".to_string()]);
        assert_eq!(texts["agg1"], "report body");
    }

    #[test]
    fn resolve_prefers_choice_then_base_then_first_final() {
        let mut pool = HashMap::new();
        pool.insert("agg1".to_string(), Rollout::delivered_ok("agg1"));
        pool.insert("base1".to_string(), Rollout::delivered_ok("base1"));
        let mut id_map = HashMap::new();
        id_map.insert("A".to_string(), "agg1".to_string());
        id_map.insert("B".to_string(), "base1".to_string());
        assert_eq!(
            resolve_choice(Some("A"), &id_map, &pool, "mixed"),
            SelectorOutcome::Chosen {
                anon: "A".to_string(),
                label: "agg1".to_string()
            }
        );
        // Bad choice with a delivered base -> neutral base fallback.
        assert_eq!(
            resolve_choice(Some("ZZZ"), &id_map, &pool, "mixed"),
            SelectorOutcome::Fallback {
                label: "base1".to_string(),
                error: "chose \"ZZZ\", which is not one of the candidates".to_string()
            }
        );
        // Finals pool without base -> first final.
        pool.remove("base1");
        assert!(matches!(
            resolve_choice(None, &id_map, &pool, "finals"),
            SelectorOutcome::Fallback { label, .. } if label == "agg1"
        ));
        // Mixed pool without base and no valid choice -> no fallback.
        assert!(matches!(
            resolve_choice(None, &id_map, &pool, "mixed"),
            SelectorOutcome::NoFallback { .. }
        ));
    }

    #[test]
    fn trivial_selection_delivers_single_final() {
        let mut cands = HashMap::new();
        cands.insert("c1".to_string(), Rollout::delivered_ok("c1"));
        let mut arts = HashMap::new();
        arts.insert("c1".to_string(), "report".to_string());
        assert_eq!(
            trivial_selection(&cands, &arts).unwrap(),
            ("A".to_string(), "c1".to_string())
        );
        // Empty pool: no base fallback in a finals-only pool.
        let empty_arts: HashMap<String, String> = HashMap::new();
        assert!(trivial_selection(&cands, &empty_arts).is_err());
        // More than one candidate is a caller bug.
        cands.insert("c2".to_string(), Rollout::delivered_ok("c2"));
        arts.insert("c2".to_string(), "other".to_string());
        assert!(trivial_selection(&cands, &arts).is_err());
    }

    #[test]
    fn experience_requires_all_fields() {
        assert!(Experience::new("", "l", "e").is_none());
        assert!(Experience::new("c", "l", "e").is_some());
    }
}
