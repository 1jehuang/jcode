//! Tier-2 atomic-judge prod gate: own bars independent of Tier-1 EM.
//!
//! Calibration (17-judge-calib REPORT, 2026-09-27) left Tier-2 report-only:
//! 47/47 gold PASS and 97.9% vague-rejection on the pinned primary, but
//! judge-judge kappa=0.0 on the backup leg (backup-model
//! instruction-incompatibility, not primary instability). Promoting Tier-2 to
//! a prod gate needs its OWN gate, not Tier-1 EM's: pass bars with 95% CIs
//! (never point estimates), abstain precision/recall, and CWR with a bar set
//! from shadow volume (never n=47).
//!
//! The kappa/CI formula here mirrors `exp/judge-runner/heads/h2_agree.py`
//! (`kappa_and_ci`, same normal-approximation SE); the h2 head stays the
//! offline ledger machinery and is reused, not duplicated. This module is the
//! programmatic gate predicate over the same quantities.
//!
//! Fail-closed end to end: thin evidence yields [`GateVerdict::Unknown`],
//! measured misses yield [`GateVerdict::NoShip`], and only a fully measured
//! pass yields [`GateVerdict::Ship`]. Tier-2 errors mid-path become
//! NEEDS-WORK records, never guessed grades and never silent drops (see
//! [`apply_tier2_to_selection`]).

use std::collections::BTreeMap;

use crate::memory::MemoryEntry;

/// Pinned Tier-2 primary judge id (P0(a) verified, calibration primary).
pub const TIER2_PRIMARY_MODEL: &str = "mimo-v2.6-flash-free";

/// Pinned Tier-2 backup judge id (P0(a) verified as judge/decomp backup).
///
/// NOTE: this is NOT the calibration backup. The calibration backup
/// (`mimo-v2.5-free`) proved instruction-incompatible (27/47 parse errors,
/// the kappa=0.0 exception; parked register lists mimo-v2.5 INCOMPATIBLE).
/// The gate pins the P0(a)-verified judge backup instead.
pub const TIER2_BACKUP_MODEL: &str = "muse-spark-1.3-contributor-free";

/// Minimum `max_tokens` for the backup id (P0(a): the provider rejects
/// max_tokens < 16 and returns empty at 16; 32 answers cleanly).
pub const TIER2_BACKUP_MIN_MAX_TOKENS: u32 = 32;

/// 2x2 agreement table: EM-correct vs judge-relevant-set-nonempty.
/// Field order matches h2_agree.py: (a, b, c, d) = (both-yes,
/// em-yes-judge-no, em-no-judge-yes, both-no).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgreementTable {
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
}

impl AgreementTable {
    pub fn n(&self) -> u64 {
        self.a + self.b + self.c + self.d
    }

    pub fn agree_rate(&self) -> Option<f64> {
        let n = self.n();
        if n == 0 {
            return None;
        }
        Some((self.a + self.d) as f64 / n as f64)
    }
}

/// Cohen kappa with 95% CI half-width. Formula mirrors h2_agree.py exactly:
/// `se = sqrt(po*(1-po) / (n*(1-pe)^2))`, CI = 1.96*se. Returns
/// `(None, None)` on empty or degenerate (pe >= 1) tables: unmeasurable,
/// never reported as agreement.
pub fn kappa_and_ci(table: AgreementTable) -> (Option<f64>, Option<f64>) {
    let (a, b, c, d) = (
        table.a as f64,
        table.b as f64,
        table.c as f64,
        table.d as f64,
    );
    let n = a + b + c + d;
    if n == 0.0 {
        return (None, None);
    }
    let po = (a + d) / n;
    let pe = ((a + b) * (a + c) + (c + d) * (b + d)) / (n * n);
    if pe >= 1.0 {
        return (None, None);
    }
    let kappa = (po - pe) / (1.0 - pe);
    let se = (po * (1.0 - po)).max(0.0).sqrt() / ((n as f64).sqrt() * (1.0 - pe));
    (Some(kappa), Some(1.96 * se))
}

/// Per-category agreement evidence: one 2x2 table plus its kappa summary.
#[derive(Debug, Clone)]
pub struct CategoryAgreement {
    pub n: u64,
    pub agree: f64,
    pub kappa: Option<f64>,
    pub kappa_ci95_half: Option<f64>,
}

impl CategoryAgreement {
    pub fn from_table(table: AgreementTable) -> Self {
        let (kappa, ci) = kappa_and_ci(table);
        let n = table.n();
        CategoryAgreement {
            n,
            agree: table.agree_rate().unwrap_or(0.0),
            kappa,
            kappa_ci95_half: ci,
        }
    }

    /// Lower bound of the 95% CI. `None` when kappa is unmeasurable.
    pub fn kappa_lower(&self) -> Option<f64> {
        match (self.kappa, self.kappa_ci95_half) {
            (Some(k), Some(ci)) => Some(k - ci),
            _ => None,
        }
    }
}

/// Pass bars for the Tier-2 gate. Every rate bar is evaluated on a CI lower
/// bound or a volume floor, never on a point estimate. The CWR bar comes
/// from shadow volume; the default is a stated conservative placeholder the
/// coordinator re-derives from shadow counts before any promotion.
#[derive(Debug, Clone)]
pub struct Tier2GateBars {
    /// Minimum kappa 95%-CI lower bound per category ("substantial").
    pub kappa_min_lower: f64,
    /// Minimum items per category before the gate may pass (never n=47).
    pub min_n_per_category: usize,
    /// Minimum abstain precision.
    pub abstain_precision_min: f64,
    /// Minimum abstained items before the abstain bar may pass.
    pub abstain_min_n: usize,
    /// Minimum abstain recall when a should-abstain gold key exists.
    pub abstain_recall_min: f64,
    /// Maximum conditional-wrongness rate P(EM wrong | judge relevant).
    pub cwr_max_rate: f64,
    /// Minimum judge-relevant rows (from SHADOW volume) for the CWR bar.
    pub cwr_min_n: usize,
    /// Maximum NEEDS-WORK rate (mirrors h6 shadow: transport not yet clean).
    pub max_needswork_rate: f64,
}

impl Default for Tier2GateBars {
    fn default() -> Self {
        Tier2GateBars {
            kappa_min_lower: 0.60,
            min_n_per_category: 100,
            abstain_precision_min: 0.90,
            abstain_min_n: 20,
            abstain_recall_min: 0.70,
            cwr_max_rate: 0.10,
            cwr_min_n: 200,
            max_needswork_rate: 0.05,
        }
    }
}

/// Abstain evidence. Recall is `None` by design when no should-abstain gold
/// key exists (mirrors h2's recall_note); missing recall never blocks, but
/// present-and-low recall does.
#[derive(Debug, Clone, Default)]
pub struct AbstainEvidence {
    pub n_abstained: usize,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub recall_missing: bool,
}

/// Gate input: the h2 quantities plus provenance. NEEDS-WORK rows are
/// already excluded from every rate by the head; `needswork_rate` gates on
/// transport cleanliness instead.
#[derive(Debug, Clone, Default)]
pub struct Tier2GateInput {
    pub per_category: BTreeMap<String, CategoryAgreement>,
    pub abstain: AbstainEvidence,
    /// CWR and its denominator (judge-relevant rows, shadow volume).
    pub cwr_rate: Option<f64>,
    pub cwr_n: usize,
    pub needswork_rate: f64,
    /// Model ids that produced the evidence (pinned-id check).
    pub models_used: Vec<String>,
}

/// Gate verdict. Only [`GateVerdict::Ship`] promotes; thin evidence is
/// [`GateVerdict::Unknown`], never a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    Ship,
    NoShip { reasons: Vec<String> },
    Unknown { reason: String },
}

impl GateVerdict {
    pub fn is_ship(&self) -> bool {
        matches!(self, GateVerdict::Ship)
    }
}

/// Evaluate the Tier-2 prod gate. Volume floors are checked before rate
/// bars so thin data yields `Unknown`, never `Ship` on a lucky point
/// estimate.
pub fn evaluate_gate(input: &Tier2GateInput, bars: &Tier2GateBars) -> GateVerdict {
    if !input.models_used.iter().any(|m| m == TIER2_PRIMARY_MODEL) {
        return GateVerdict::NoShip {
            reasons: vec![format!(
                "evidence not from pinned primary {TIER2_PRIMARY_MODEL}"
            )],
        };
    }
    if input
        .models_used
        .iter()
        .any(|m| m != TIER2_PRIMARY_MODEL && m != TIER2_BACKUP_MODEL)
    {
        return GateVerdict::NoShip {
            reasons: vec!["evidence uses an unpinned model id".to_string()],
        };
    }
    if input.needswork_rate > bars.max_needswork_rate {
        return GateVerdict::NoShip {
            reasons: vec![format!(
                "needswork rate {:.3} above max {:.3} (transport not clean)",
                input.needswork_rate, bars.max_needswork_rate
            )],
        };
    }
    if input.per_category.is_empty() {
        return GateVerdict::Unknown {
            reason: "no per-category agreement evidence".to_string(),
        };
    }
    let mut failures: Vec<String> = Vec::new();
    for (name, cat) in &input.per_category {
        if (cat.n as usize) < bars.min_n_per_category {
            return GateVerdict::Unknown {
                reason: format!(
                    "category {name}: n={} below floor {} (shadow volume required, never n=47)",
                    cat.n, bars.min_n_per_category
                ),
            };
        }
        match cat.kappa_lower() {
            None => {
                return GateVerdict::Unknown {
                    reason: format!(
                        "category {name}: kappa unmeasurable (degenerate table); reported as unmeasurable, not agreement"
                    ),
                };
            }
            Some(lower) => {
                if lower < bars.kappa_min_lower {
                    failures.push(format!(
                        "category {name}: kappa lower bound {lower:.3} below bar {:.3}",
                        bars.kappa_min_lower
                    ));
                }
            }
        }
    }
    if input.abstain.n_abstained < bars.abstain_min_n {
        return GateVerdict::Unknown {
            reason: format!(
                "abstain: n={} below floor {}",
                input.abstain.n_abstained, bars.abstain_min_n
            ),
        };
    }
    match input.abstain.precision {
        None => {
            return GateVerdict::Unknown {
                reason: "abstain precision unmeasurable (no abstained rows with gold)".to_string(),
            };
        }
        Some(p) => {
            if p < bars.abstain_precision_min {
                failures.push(format!(
                    "abstain precision {p:.3} below bar {:.3}",
                    bars.abstain_precision_min
                ));
            }
        }
    }
    if !input.abstain.recall_missing {
        if let Some(r) = input.abstain.recall {
            if r < bars.abstain_recall_min {
                failures.push(format!(
                    "abstain recall {r:.3} below bar {:.3}",
                    bars.abstain_recall_min
                ));
            }
        }
    }
    if input.cwr_n < bars.cwr_min_n {
        return GateVerdict::Unknown {
            reason: format!(
                "CWR: n={} below shadow-volume floor {} (bar from shadow volume, never n=47)",
                input.cwr_n, bars.cwr_min_n
            ),
        };
    }
    match input.cwr_rate {
        None => {
            return GateVerdict::Unknown {
                reason: "CWR unmeasurable (no judge-relevant rows)".to_string(),
            };
        }
        Some(rate) => {
            if rate > bars.cwr_max_rate {
                failures.push(format!(
                    "CWR {rate:.3} above bar {:.3}",
                    bars.cwr_max_rate
                ));
            }
        }
    }
    if failures.is_empty() {
        GateVerdict::Ship
    } else {
        GateVerdict::NoShip { reasons: failures }
    }
}

/// Per-entry Tier-2 verdict over a Jev-selected memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier2Verdict {
    /// Atom support verified: the selection stands.
    Pass,
    /// Support check failed: veto is explicit and recorded, never silent.
    Fail,
    /// Judge errored (transport/parse/timeout): NEEDS-WORK, never a grade.
    Tier2Error,
}

/// Outcome of overlaying Tier-2 verdicts on a Jev selection. Every input
/// entry lands in exactly one of the three lists: kept, vetoed (explicit
/// disagreement), or needs-work (error or missing verdict). There is no
/// silent fourth outcome.
#[derive(Debug)]
pub struct Tier2InteractionOutcome {
    pub kept: Vec<(MemoryEntry, f32)>,
    pub vetoed: Vec<String>,
    pub needs_work: Vec<String>,
}

impl Tier2InteractionOutcome {
    /// True when any entry needs Tier-2 re-grade: the interaction degraded
    /// (counts toward the degradation attribution, never fails open).
    pub fn is_degraded(&self) -> bool {
        !self.needs_work.is_empty()
    }
}

/// Overlay Tier-2 verdicts on a Jev-selected set.
///
/// Fail-closed rules:
/// - `Fail` removes the entry AND records the veto (explicit handling).
/// - `Tier2Error` or a missing verdict records NEEDS-WORK (never assumed
///   pass, never silent drop, never silent keep).
/// - Jev-side transport failure still aborts selection upstream
///   (`memory_jev::select_with_transport` propagates the error and keeps
///   nothing partial); this overlay only sees successfully selected sets.
pub fn apply_tier2_to_selection(
    selected: Vec<(MemoryEntry, f32)>,
    verdicts: &BTreeMap<String, Tier2Verdict>,
) -> Tier2InteractionOutcome {
    let mut out = Tier2InteractionOutcome {
        kept: Vec::new(),
        vetoed: Vec::new(),
        needs_work: Vec::new(),
    };
    for (entry, score) in selected {
        match verdicts.get(&entry.id) {
            Some(Tier2Verdict::Pass) => out.kept.push((entry, score)),
            Some(Tier2Verdict::Fail) => out.vetoed.push(entry.id),
            Some(Tier2Verdict::Tier2Error) | None => out.needs_work.push(entry.id),
        }
    }
    out
}

/// Atom-order swap agreement: the pointwise Tier-2 analogue of h4 swap.
///
/// h4 exempts pointwise Tier-2 ("no candidate order exists there"). The
/// exemption is closed here, not by reusing candidate-order swap (vacuous
/// for pointwise grading) but by the order-swap that DOES exist on the
/// Tier-2 leg: the gold atoms are presented in listed order, so grading the
/// same item with the atom order reversed must yield the same verdict.
/// Returns `None` when either grade errored (NEEDS-WORK never agrees,
/// mirroring h4), `Some(agree)` otherwise.
pub fn atom_order_swap_agree(base: Tier2Verdict, swapped: Tier2Verdict) -> Option<bool> {
    match (base, swapped) {
        (Tier2Verdict::Tier2Error, _) | (_, Tier2Verdict::Tier2Error) => None,
        _ => Some(base == swapped),
    }
}

/// Swap-agreement rate over (base, swapped) verdict pairs. Error pairs are
/// excluded from the rate and counted separately.
pub fn swap_agreement_rate(pairs: &[(Tier2Verdict, Tier2Verdict)]) -> (usize, usize, usize) {
    let mut agree = 0usize;
    let mut total = 0usize;
    let mut excluded = 0usize;
    for (base, swapped) in pairs {
        match atom_order_swap_agree(*base, *swapped) {
            None => excluded += 1,
            Some(true) => {
                agree += 1;
                total += 1;
            }
            Some(false) => total += 1,
        }
    }
    (agree, total, excluded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryCategory;

    fn cat(a: u64, b: u64, c: u64, d: u64) -> CategoryAgreement {
        CategoryAgreement::from_table(AgreementTable { a, b, c, d })
    }

    fn passing_input() -> Tier2GateInput {
        let mut per_category = BTreeMap::new();
        per_category.insert("C1-K".to_string(), cat(110, 1, 1, 8));
        per_category.insert("C1-P".to_string(), cat(105, 2, 1, 12));
        Tier2GateInput {
            per_category,
            abstain: AbstainEvidence {
                n_abstained: 24,
                precision: Some(0.95),
                recall: Some(0.80),
                recall_missing: false,
            },
            cwr_rate: Some(0.05),
            cwr_n: 240,
            needswork_rate: 0.02,
            models_used: vec![TIER2_PRIMARY_MODEL.to_string()],
        }
    }

    #[test]
    fn kappa_mirrors_h2_on_synthetic_tables() {
        // Values pinned to h2_agree.py output on the synthetic smoke ledger:
        // C1-K (2,1,1,2) -> k=1/3, ci=0.754404351741111;
        // C1-P (3,1,0,2) -> k=2/3, ci=0.5964090070611808.
        let (k, ci) = kappa_and_ci(AgreementTable { a: 2, b: 1, c: 1, d: 2 });
        assert!((k.unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert!((ci.unwrap() - 0.754404351741111).abs() < 1e-9);
        let (k, ci) = kappa_and_ci(AgreementTable { a: 3, b: 1, c: 0, d: 2 });
        assert!((k.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert!((ci.unwrap() - 0.5964090070611808).abs() < 1e-9);
    }

    #[test]
    fn kappa_degenerate_is_unmeasurable_not_agreement() {
        assert_eq!(
            kappa_and_ci(AgreementTable { a: 0, b: 0, c: 0, d: 0 }),
            (None, None)
        );
        // pe = 1 (all mass in one marginal): unmeasurable.
        assert_eq!(
            kappa_and_ci(AgreementTable { a: 5, b: 0, c: 0, d: 0 }),
            (None, None)
        );
    }

    #[test]
    fn gate_ships_on_fully_measured_pass() {
        assert_eq!(
            evaluate_gate(&passing_input(), &Tier2GateBars::default()),
            GateVerdict::Ship
        );
    }

    #[test]
    fn gate_never_ships_on_thin_data_even_when_perfect() {
        // Perfect agreement at n=47 (calibration volume): bars pass, but the
        // volume floor forces Unknown, never Ship.
        let mut input = passing_input();
        input
            .per_category
            .insert("C1-K".to_string(), cat(45, 0, 0, 2));
        assert_eq!(
            evaluate_gate(&input, &Tier2GateBars::default()),
            GateVerdict::Unknown {
                reason: "category C1-K: n=47 below floor 100 (shadow volume required, never n=47)"
                    .to_string(),
            }
        );
    }

    #[test]
    fn gate_no_ship_on_kappa_lower_bound_miss() {
        let mut input = passing_input();
        // Weak agreement at full volume: lower bound misses the bar.
        input
            .per_category
            .insert("C1-K".to_string(), cat(40, 30, 30, 20));
        let verdict = evaluate_gate(&input, &Tier2GateBars::default());
        assert!(matches!(verdict, GateVerdict::NoShip { .. }), "{verdict:?}");
        if let GateVerdict::NoShip { reasons } = verdict {
            assert!(reasons.iter().any(|r| r.contains("C1-K")), "{reasons:?}");
        }
    }

    #[test]
    fn gate_cwr_bar_from_shadow_volume() {
        let mut input = passing_input();
        input.cwr_rate = Some(0.15);
        assert!(matches!(
            evaluate_gate(&input, &Tier2GateBars::default()),
            GateVerdict::NoShip { .. }
        ));
        // Same rate shape at calibration volume: Unknown, not a pass.
        let mut thin = passing_input();
        thin.cwr_n = 47;
        thin.cwr_rate = Some(0.01);
        assert!(matches!(
            evaluate_gate(&thin, &Tier2GateBars::default()),
            GateVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn gate_no_ship_on_dirty_transport() {
        let mut input = passing_input();
        input.needswork_rate = 0.08;
        assert!(matches!(
            evaluate_gate(&input, &Tier2GateBars::default()),
            GateVerdict::NoShip { .. }
        ));
    }

    #[test]
    fn gate_rejects_unpinned_model_ids() {
        let mut input = passing_input();
        input.models_used = vec!["mimo-v2.5-free".to_string()];
        let verdict = evaluate_gate(&input, &Tier2GateBars::default());
        assert!(matches!(verdict, GateVerdict::NoShip { .. }), "{verdict:?}");
    }

    #[test]
    fn pinned_ids_match_p0a_verification() {
        assert_eq!(TIER2_PRIMARY_MODEL, "mimo-v2.6-flash-free");
        assert_eq!(TIER2_BACKUP_MODEL, "muse-spark-1.3-contributor-free");
        assert!(TIER2_BACKUP_MIN_MAX_TOKENS >= 32);
    }

    #[test]
    fn tier2_disagree_is_explicit_veto_never_silent() {
        let entries = vec![
            (mem("m1"), 0.95),
            (mem("m2"), 0.90),
            (mem("m3"), 0.85),
        ];
        let verdicts = BTreeMap::from([
            ("m1".to_string(), Tier2Verdict::Pass),
            ("m2".to_string(), Tier2Verdict::Fail),
            ("m3".to_string(), Tier2Verdict::Tier2Error),
        ]);
        let out = apply_tier2_to_selection(entries, &verdicts);
        assert_eq!(out.kept.len(), 1);
        assert_eq!(out.kept[0].0.id, "m1");
        // Disagreement is recorded, not silently dropped.
        assert_eq!(out.vetoed, vec!["m2".to_string()]);
        // Error is NEEDS-WORK, and the interaction counts as degraded.
        assert_eq!(out.needs_work, vec!["m3".to_string()]);
        assert!(out.is_degraded());
    }

    #[test]
    fn tier2_missing_verdict_is_needswork_never_assumed_pass() {
        let entries = vec![(mem("m1"), 0.95)];
        let out = apply_tier2_to_selection(entries, &BTreeMap::new());
        assert!(out.kept.is_empty());
        assert_eq!(out.needs_work, vec!["m1".to_string()]);
        assert!(out.is_degraded());
    }

    #[test]
    fn tier2_all_pass_keeps_everything_with_no_records() {
        let entries = vec![(mem("m1"), 0.95), (mem("m2"), 0.90)];
        let verdicts = BTreeMap::from([
            ("m1".to_string(), Tier2Verdict::Pass),
            ("m2".to_string(), Tier2Verdict::Pass),
        ]);
        let out = apply_tier2_to_selection(entries, &verdicts);
        assert_eq!(out.kept.len(), 2);
        assert!(out.vetoed.is_empty());
        assert!(out.needs_work.is_empty());
        assert!(!out.is_degraded());
    }

    #[test]
    fn atom_order_swap_agreement_excludes_needswork() {
        use Tier2Verdict::{Fail, Pass, Tier2Error};
        assert_eq!(atom_order_swap_agree(Pass, Pass), Some(true));
        assert_eq!(atom_order_swap_agree(Pass, Fail), Some(false));
        assert_eq!(atom_order_swap_agree(Fail, Fail), Some(true));
        assert_eq!(atom_order_swap_agree(Tier2Error, Pass), None);
        assert_eq!(atom_order_swap_agree(Pass, Tier2Error), None);
        let (agree, total, excluded) = swap_agreement_rate(&[
            (Pass, Pass),
            (Pass, Fail),
            (Tier2Error, Pass),
        ]);
        assert_eq!((agree, total, excluded), (1, 2, 1));
    }

    #[tokio::test]
    async fn jev_select_then_tier2_overlay_is_measured_end_to_end() {
        // Jev side: mock transport scores "relevant" content 0.95, else 0.2.
        let entries = vec![
            mem_content("m1", "relevant budget decision"),
            mem_content("m2", "relevant but tier2 disputes it"),
            mem_content("m3", "relevant but tier2 errors on it"),
            mem_content("m4", "unrelated trivia"),
        ];
        let selected = crate::memory_jev::select_with_transport(
            &MockTransport,
            "what is the budget decision",
            entries,
            5,
            0.8,
        )
        .await
        .expect("mock transport never fails");
        let ids: Vec<&str> = selected.iter().map(|(e, _)| e.id.as_str()).collect();
        // Jev filters m4 (0.2 < 0.8) and keeps the three relevant ones.
        assert_eq!(ids, vec!["m1", "m2", "m3"]);
        // Tier-2 mid-path: one pass, one disagree, one error.
        let verdicts = BTreeMap::from([
            ("m1".to_string(), Tier2Verdict::Pass),
            ("m2".to_string(), Tier2Verdict::Fail),
            ("m3".to_string(), Tier2Verdict::Tier2Error),
        ]);
        let out = apply_tier2_to_selection(selected, &verdicts);
        assert_eq!(out.kept.len(), 1);
        assert_eq!(out.kept[0].0.id, "m1");
        assert_eq!(out.vetoed, vec!["m2".to_string()]);
        assert_eq!(out.needs_work, vec!["m3".to_string()]);
        assert!(out.is_degraded());
    }

    fn mem(id: &str) -> MemoryEntry {
        let mut e = MemoryEntry::new(MemoryCategory::Fact, "test memory");
        e.id = id.to_string();
        e
    }

    fn mem_content(id: &str, content: &str) -> MemoryEntry {
        let mut e = MemoryEntry::new(MemoryCategory::Fact, content);
        e.id = id.to_string();
        e
    }

    struct MockTransport;

    #[async_trait::async_trait]
    impl crate::memory_jev::RelevanceTransport for MockTransport {
        async fn evaluate(
            &self,
            state: serde_json::Value,
            questions: serde_json::Map<String, serde_json::Value>,
        ) -> anyhow::Result<serde_json::Value> {
            let answers: serde_json::Map<String, serde_json::Value> = questions
                .keys()
                .map(|key| {
                    let content = state["candidates"][key]["content"]
                        .as_str()
                        .unwrap_or("");
                    let score = if content.contains("relevant") {
                        0.95
                    } else {
                        0.2
                    };
                    (
                        key.clone(),
                        serde_json::json!({"type": "noul", "noul": score}),
                    )
                })
                .collect();
            Ok(serde_json::json!({"answers": answers}))
        }
    }
}
