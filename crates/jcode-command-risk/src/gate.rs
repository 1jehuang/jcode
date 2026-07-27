//! Stage 2: the reflection gate.
//!
//! Stage 1 ([`crate::assess`]) decides *whether* a command deserves scrutiny.
//! This decides *what happens next*, and it is deliberately not another model.
//!
//! # Why not a second model
//!
//! An LLM judge is expensive, adds latency to every borderline call, and can be
//! talked around by the same reasoning that produced the command. It also
//! creates a second thing to keep aligned.
//!
//! Instead the harness refuses once and hands back a structured prompt that
//! forces the *generating* model to do the thinking it skipped. The key
//! property is that the refusal is not satisfiable by repetition: the model must
//! supply a `justification` naming what the user actually asked for. A blind
//! retry of the identical call fails again.
//!
//! This is spending test-time compute on alignment, but at the point of action
//! and with no extra model in the loop.

use crate::{RiskAssessment, RiskLevel};

/// What the harness should do with a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Run it.
    Allow,
    /// Refuse this attempt and make the model justify itself first.
    Reflect { prompt: String },
    /// Refuse permanently. No justification unlocks this.
    Deny { reason: String },
}

/// A justification supplied by the model on a second attempt.
#[derive(Debug, Clone, Default)]
pub struct Justification {
    /// The model's account of which user request this serves.
    pub text: Option<String>,
}

impl Justification {
    /// Whether the justification is substantive rather than a token retry.
    ///
    /// This is intentionally a low bar: the goal is to force a reflection turn,
    /// not to grade prose. Anything that shows the model actually re-read the
    /// request passes; an empty string or a bare "yes" does not.
    pub fn is_substantive(&self) -> bool {
        let Some(text) = self.text.as_deref().map(str::trim) else {
            return false;
        };
        if text.len() < MIN_JUSTIFICATION_LEN {
            return false;
        }
        // Reject pure affirmations, which carry no information about intent.
        const EMPTY_AFFIRMATIONS: &[&str] = &[
            "yes",
            "ok",
            "okay",
            "sure",
            "confirmed",
            "proceed",
            "do it",
            "continue",
            "y",
            "approved",
            "go ahead",
        ];
        let lowered = text.to_lowercase();
        let stripped = lowered.trim_end_matches(['.', '!', '?']).trim();
        !EMPTY_AFFIRMATIONS.contains(&stripped)
    }
}

/// Minimum characters for a justification to count as considered.
const MIN_JUSTIFICATION_LEN: usize = 25;

/// Decide what to do with an assessed command.
pub fn gate(assessment: &RiskAssessment, justification: &Justification) -> GateOutcome {
    match assessment.level {
        RiskLevel::Safe | RiskLevel::Low => GateOutcome::Allow,
        RiskLevel::Catastrophic => GateOutcome::Deny {
            reason: format!(
                "This command is blocked and cannot be confirmed.\n\n{}\n\
                 If the user genuinely wants this, they must run it themselves \
                 outside the agent.",
                assessment.explanation()
            ),
        },
        RiskLevel::Confirm => {
            if justification.is_substantive() {
                GateOutcome::Allow
            } else {
                GateOutcome::Reflect {
                    prompt: reflection_prompt(assessment),
                }
            }
        }
    }
}

/// The text handed back to the model on a refused first attempt.
///
/// Phrasing is deliberate: it asks the model to compare the command against
/// **what the user actually asked for**, which is a checkable claim about a
/// specific message, rather than a vague appeal to good judgement that invites
/// a reflexive "yes, this is fine".
fn reflection_prompt(assessment: &RiskAssessment) -> String {
    format!(
        "This command was not run. It is irreversible:\n\n{}\n\
         Before it can proceed, stop and check it against the user's actual \
         request:\n\
         - Which specific thing the user asked for requires deleting this?\n\
         - Did the user name this path, or did you infer it?\n\
         - If you inferred it, is a narrower target enough?\n\
         - If this is wrong, nothing here can be recovered.\n\n\
         If it is genuinely what the user asked for, re-issue the same call \
         with a `justification` field explaining which request it serves. If \
         you are not sure, ask the user instead: that costs one message, and \
         being wrong costs their data.",
        assessment.explanation()
    )
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod gate_tests;
