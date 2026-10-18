//! Harness refinement primitives ported from harness2's proven design.
//!
//! harness2 (`google-research/harness2`) is a *meta* harness: it improves other
//! harnesses per task via Propose-Probe-Compose recursion, anonymized
//! selection, and a per-domain experience library. jcode is an *execution*
//! harness: it runs the agents. These primitives bring the portable,
//! executor-independent half of harness2 into `jcode-plan` so a future
//! `jcode refine --k` loop can reuse validated logic instead of re-inventing it.
//!
//! What lives here (and what deliberately does not):
//! - [`surface`]: the editable c1..c8 component tree + validators.
//! - [`recursion`]: task-level recursion budgets (parallel/sequential), stage
//!   labels, finals-only pool construction.
//! - [`selection`]: anonymized, seeded-shuffle candidate choice with safe
//!   fallbacks.
//! - [`library`]: per-domain experience library (full tree + why it won).
//! - [`hygiene`]: eval-hygiene guards (no grader verdicts, no leak paths,
//!   observation truncation floor).
//!
//! No executor, model, network, or filesystem-IO lives here: callers supply
//! trees as `HashMap<rel, content>` and persist what these helpers return.
//! That keeps this crate dependency-free (serde only) and fully unit-testable.

pub mod hygiene;
pub mod library;
pub mod recursion;
pub mod selection;
pub mod surface;
