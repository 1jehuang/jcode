//! Session-end episode recorder for transcript rule proposals (R3 wiring).
//!
//! Scans closing transcripts for HIGH-SIGNAL patterns only and records
//! episodes into the [`RuleBuffer`]. Deliberately conservative: an empty
//! buffer is fine (nothing to propose yet), a garbage buffer poisons
//! Phase-2 wordsmithing. Every pattern requires an explicit marker phrase;
//! casual "no", "thanks", "wrong file?" never match.
//!
//! Noise history (2026-10-04, 2,474-episode audit): three compounding
//! defects flooded the buffer — triple-append on every re-close, tool/hook
//! boilerplate matching preference markers, and procedure runs of one tool
//! name carrying zero actionable content. Fixes, in audit order:
//! - P0-1 idempotency: `already_recorded` skips sessions already present.
//! - P0-3 user-authored-only gate: skip system-reminder blocks and long
//!   tool-description-shaped texts before marker matching.
//! - P0-2 procedure drop: `scan_tool_runs` stays only as a shim returning
//!   nothing; same-name runs never become standing rules (no goal, no
//!   context, no prescription). Kept (not deleted) so the call site and old
//!   tests read as intent, and a future heterogeneous-sequence detector has
//!   a home.
//! Fail-open everywhere: I/O errors, missing dirs, and empty transcripts
//! record nothing and change nothing.

use crate::transcript_rules::{Episode, RuleBuffer, RuleKind, RuleScope};

/// On-disk file name for the episode buffer under `~/.jcode/rules/`.
pub const BUFFER_FILE_NAME: &str = "rule-buffer.json";

/// User-role texts at or above this char count are never user-authored:
/// they are pasted tool-description JSON, system-reminder walls, or audit
/// boilerplate. Real corrections/preferences are short; the audit's top
/// noise trigger lived inside a 38KB system-reminder block.
pub const MAX_USER_AUTHORED_TEXT_CHARS: usize = 2000;

/// Correction markers: explicit wrongness + replacement signal. Each must
/// appear as a substring (case-insensitive) in user text. Bare "no" or
/// "wrong" alone never match — too noisy.
const CORRECTION_MARKERS: &[&str] = &[
    "that's wrong",
    "thats wrong",
    "that is wrong",
    "you got it wrong",
    "you're wrong",
    "you are wrong",
    "no, it should be",
    "no it should be",
    "should actually be",
    "actually it should be",
    "not that, ",
    "i meant ",
    "i said ",
];

/// Preference markers: durable instruction signal, not one-off requests.
/// "please ..." alone never matches — politeness is not preference.
const PREFERENCE_MARKERS: &[&str] = &[
    "always ",
    "never ",
    "from now on",
    "prefer ",
    "don't do that again",
    "dont do that again",
    "do not do that again",
    "remember to always",
    "remember to never",
];

/// Scan user text messages for episodes. Returns episodes in transcript
/// order. Pure function — no I/O, no globals.
///
/// P0-3 user-authored-only gate: skip texts that cannot be the user's own
/// words — `<system-reminder>` blocks (MCP tool descriptions ride inside
/// them under the user role) and texts at/over
/// [`MAX_USER_AUTHORED_TEXT_CHARS`] (pasted JSON/schema walls, audit
/// boilerplate). The audit's top noise trigger (fetch tool description,
/// "never use focus", 10/10 sessions) dies here; short user corrections
/// and preferences pass through.
pub fn scan_user_text(
    user_texts: &[String],
    session_id: &str,
    scope: RuleScope,
) -> Vec<Episode> {
    let mut episodes = Vec::new();
    for text in user_texts {
        if !is_user_authored(text) {
            continue;
        }
        let lower = text.to_lowercase();
        if let Some(trigger) = first_correction_trigger(&lower) {
            episodes.push(Episode::now(
                RuleKind::Correction,
                scope,
                session_id,
                &trigger,
            ));
        } else if let Some(trigger) = first_preference_trigger(&lower) {
            episodes.push(Episode::now(
                RuleKind::Preference,
                scope,
                session_id,
                &trigger,
            ));
        }
    }
    episodes
}

/// True when a user-role text could be the user's own words. Rejects the
/// two shapes the audit proved are machine text wearing the user role:
/// `<system-reminder>` blocks (hook/tool injections) and long pasted walls
/// (tool-description JSON, `input_schema` shapes, audit boilerplate).
/// Char count (not bytes): CJK-heavy pastes must not slip under a byte cap.
fn is_user_authored(text: &str) -> bool {
    if text.contains("<system-reminder>") {
        return false;
    }
    if text.contains("input_schema") {
        return false;
    }
    text.chars().count() < MAX_USER_AUTHORED_TEXT_CHARS
}

/// The sentence containing the first correction marker, clipped to the
/// trigger budget. `None` when no marker matches.
fn first_correction_trigger(lower: &str) -> Option<String> {
    first_marker_sentence(lower, CORRECTION_MARKERS)
}

/// The sentence containing the first preference marker. `None` when none.
fn first_preference_trigger(lower: &str) -> Option<String> {
    first_marker_sentence(lower, PREFERENCE_MARKERS)
}

/// Find the first marker hit and return its enclosing sentence.
fn first_marker_sentence(lower: &str, markers: &[&str]) -> Option<String> {
    let hit = markers.iter().find(|m| lower.contains(*m))?;
    let pos = lower.find(hit)?;
    Some(clip_sentence(lower, pos))
}

/// Extract the sentence around `pos`: back to the previous `.`/`!`/`?`/`\n`,
/// forward to the next one. Bounded so triggers stay short patterns.
fn clip_sentence(text: &str, pos: usize) -> String {
    let bytes = text.as_bytes();
    let mut start = pos;
    while start > 0
        && !matches!(bytes[start - 1], b'.' | b'!' | b'?' | b'\n')
    {
        start -= 1;
    }
    let mut end = pos;
    while end < bytes.len()
        && !matches!(bytes[end], b'.' | b'!' | b'?' | b'\n')
    {
        end += 1;
    }
    text[start..end.min(text.len())].trim().to_string()
}

/// Scan assistant tool-call name sequences for procedure runs.
///
/// P0-2 DROP (2026-10-04 audit): same-name runs never record. A trigger of
/// "run of N bash calls" names no goal, no context, no prescription — it
/// can never become a standing rule, so identical triggers recur across
/// every session by construction and deterministically trip the N>=3
/// promotion bar. The audit: 96.1% of a 2,474-episode buffer was this
/// channel. A future heterogeneous-sequence detector (A→B→C, a real "way
/// of doing something") belongs here; until then this returns nothing.
/// Signature kept so the call site reads as intent and old tests pin it.
pub fn scan_tool_runs(
    _tool_names: &[String],
    _session_id: &str,
    _scope: RuleScope,
) -> Vec<Episode> {
    Vec::new()
}

/// Load the persisted buffer, append `episodes`, save back. Fail-open:
/// any I/O or parse error keeps the old file (or nothing) and reports
/// `Ok(0)`. Returns the number of episodes appended.
///
/// P0-1 idempotency: a session that already contributed is skipped whole.
/// Episodes are transcript-deterministic (same input rescans to the same
/// output), so a re-close appends only duplicates — the audit's mega-session
/// held 3.1x its transcript via three closes. Fail-open: an unreadable
/// buffer records (better a possible dup than a lost correction).
pub fn append_episodes(episodes: &[Episode]) -> usize {
    if episodes.is_empty() {
        return 0;
    }
    let path = match buffer_path() {
        Some(p) => p,
        None => return 0,
    };
    let mut buffer = load_buffer(&path).unwrap_or_default();
    let session_id = episodes
        .first()
        .map(|episode| episode.session_id.as_str())
        .unwrap_or("");
    if already_recorded(&buffer, session_id) {
        return 0;
    }
    for episode in episodes {
        buffer.record(episode.clone());
    }
    if save_buffer(&path, &buffer).is_err() {
        return 0;
    }
    episodes.len()
}

/// True when the buffer already holds episodes for this session (any kind).
/// Mixed-session batches never occur (one scan = one session), so the
/// first episode's session id represents the batch.
fn already_recorded(buffer: &RuleBuffer, session_id: &str) -> bool {
    !session_id.is_empty() && !buffer.episodes_for_session(session_id).is_empty()
}

/// Resolve `~/.jcode/rules/rule-buffer.json`. `None` when the home dir
/// cannot be resolved (fail-open: record nothing).
fn buffer_path() -> Option<std::path::PathBuf> {
    let dir = crate::storage::jcode_dir().ok()?;
    Some(dir.join("rules").join(BUFFER_FILE_NAME))
}

fn load_buffer(path: &std::path::Path) -> anyhow::Result<RuleBuffer> {
    let data = std::fs::read(path)?;
    let episodes: Vec<Episode> = serde_json::from_slice(&data)?;
    let mut buffer = RuleBuffer::new();
    for episode in episodes {
        buffer.record(episode);
    }
    Ok(buffer)
}

fn save_buffer(path: &std::path::Path, buffer: &RuleBuffer) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(buffer.episodes())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> RuleScope {
        RuleScope::Project
    }

    #[test]
    fn correction_markers_hit_explicit_wrongness() {
        let texts = vec!["That's wrong, the port should be 8080.".to_string()];
        let got = scan_user_text(&texts, "s1", project());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, RuleKind::Correction);
    }

    #[test]
    fn casual_no_and_thanks_never_match() {
        let texts = vec![
            "no worries, thanks!".to_string(),
            "no, let me check that file".to_string(),
            "wrong file? let me look".to_string(),
            "please show me the logs".to_string(),
            "can you explain this error".to_string(),
        ];
        let got = scan_user_text(&texts, "s1", project());
        assert!(got.is_empty(), "casual text must not propose: {got:?}");
    }

    #[test]
    fn preference_markers_hit_durable_instruction() {
        let texts = vec!["From now on always use fish shell.".to_string()];
        let got = scan_user_text(&texts, "s1", project());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, RuleKind::Preference);
    }

    #[test]
    fn politeness_alone_never_matches() {
        let texts = vec![
            "please run the tests".to_string(),
            "could you check this?".to_string(),
            "i like fish shell".to_string(),
        ];
        let got = scan_user_text(&texts, "s1", project());
        assert!(got.is_empty(), "politeness must not propose: {got:?}");
    }

    #[test]
    fn one_episode_per_text_first_marker_wins() {
        let texts = vec!["That's wrong. Never do that again.".to_string()];
        let got = scan_user_text(&texts, "s1", project());
        assert_eq!(got.len(), 1, "correction takes precedence");
        assert_eq!(got[0].kind, RuleKind::Correction);
    }

    #[test]
    fn same_name_runs_record_nothing_p0_2_drop() {
        // P0-2: any same-name run (3, 6, 10) records nothing — the trigger
        // carries no actionable content, so it can never become a rule.
        for name in ["read", "bash", "bg", "mcp_call"] {
            let names = vec![name.to_string(); 10];
            assert!(
                scan_tool_runs(&names, "s1", project()).is_empty(),
                "{name} runs must not record"
            );
        }
        let mixed = vec![
            "read".to_string(),
            "bash".to_string(),
            "bash".to_string(),
            "bash".to_string(),
            "read".to_string(),
        ];
        assert!(scan_tool_runs(&mixed, "s1", project()).is_empty());
    }

    #[test]
    fn system_reminder_and_long_texts_are_not_user_authored() {
        // P0-3: the audit's top noise shape — fetch tool-description text
        // (with a "never" marker) inside a system-reminder wall — is skipped.
        let reminder = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            "misses drop content the page's own ctrl-f would find \
             (focus is semantic, not literal) — never use focus when the \
             full content is the deliverable"
        );
        assert!(!is_user_authored(&reminder));
        assert!(scan_user_text(&[reminder], "s1", project()).is_empty());
        // Long pasted walls (tool JSON / audit boilerplate) are skipped even
        // without a reminder tag.
        let long = format!("please always do this. {}", "x".repeat(3000));
        assert!(!is_user_authored(&long));
        assert!(scan_user_text(&[long], "s1", project()).is_empty());
        // Short genuine user text still passes.
        assert!(is_user_authored("From now on always use fish shell."));
    }

    #[test]
    fn short_correction_and_preference_still_fire() {
        // P0-3 must not kill the channels the design needs: the audit's 3
        // genuine corrections + short preferences keep working.
        let texts = vec!["i meant our fork, that's wrong".to_string()];
        let got = scan_user_text(&texts, "s1", project());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, RuleKind::Correction);
        let texts = vec!["never force-push to main".to_string()];
        let got = scan_user_text(&texts, "s1", project());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, RuleKind::Preference);
    }

    #[test]
    fn already_recorded_session_skips_second_append() {
        // P0-1: same session twice → second batch contributes nothing.
        let mut buffer = RuleBuffer::new();
        buffer.record(Episode::now(
            RuleKind::Correction,
            project(),
            "s-dup",
            "that's wrong",
        ));
        assert!(already_recorded(&buffer, "s-dup"));
        assert!(!already_recorded(&buffer, "s-fresh"));
        assert!(!already_recorded(&buffer, ""));
        assert!(!already_recorded(&RuleBuffer::new(), "s-dup"));
    }

    #[test]
    fn empty_inputs_propose_nothing() {
        assert!(scan_user_text(&[], "s1", project()).is_empty());
        assert!(scan_tool_runs(&[], "s1", project()).is_empty());
    }
}
