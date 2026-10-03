//! Session-end episode recorder for transcript rule proposals (R3 wiring).
//!
//! Scans closing transcripts for HIGH-SIGNAL patterns only and records
//! episodes into the [`RuleBuffer`]. Deliberately conservative: an empty
//! buffer is fine (nothing to propose yet), a garbage buffer poisons
//! Phase-2 wordsmithing. Every pattern requires an explicit marker phrase;
//! casual "no", "thanks", "wrong file?" never match.
//!
//! Patterns (all case-insensitive, user-role text only):
//! - Correction: "that's wrong", "no, ... should be", "you got X wrong",
//!   "actually, ...", "not X, Y" corrections with explicit replacement.
//! - Preference: "always ...", "never ...", "from now on ...", "prefer ...",
//!   "don't ... again".
//! - Procedure: repeated identical tool-call sequences (>=3 same-name calls
//!   in a row) — a multi-step way worth proposing as a rule.
//! Fail-open everywhere: I/O errors, missing dirs, and empty transcripts
//! record nothing and change nothing.

use crate::transcript_rules::{Episode, RuleBuffer, RuleKind, RuleScope};

/// On-disk file name for the episode buffer under `~/.jcode/rules/`.
pub const BUFFER_FILE_NAME: &str = "rule-buffer.json";

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

/// Minimum tool-call run length to propose a procedure episode.
const PROCEDURE_RUN_LEN: usize = 3;

/// Scan user text messages for episodes. Returns episodes in transcript
/// order. Pure function — no I/O, no globals.
pub fn scan_user_text(
    user_texts: &[String],
    session_id: &str,
    scope: RuleScope,
) -> Vec<Episode> {
    let mut episodes = Vec::new();
    for text in user_texts {
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

/// Scan assistant tool-call name sequences for procedure runs: >=3
/// consecutive calls with the same tool name. Returns one episode per run
/// (trigger = "run of N <tool> calls"). Pure function.
pub fn scan_tool_runs(
    tool_names: &[String],
    session_id: &str,
    scope: RuleScope,
) -> Vec<Episode> {
    let mut episodes = Vec::new();
    if tool_names.len() < PROCEDURE_RUN_LEN {
        return episodes;
    }
    let mut run_name = &tool_names[0];
    let mut run_len = 1usize;
    let mut flush = |name: &str, len: usize, out: &mut Vec<Episode>| {
        if len >= PROCEDURE_RUN_LEN {
            out.push(Episode::now(
                RuleKind::Procedure,
                scope,
                session_id,
                &format!("run of {len} {name} calls"),
            ));
        }
    };
    for name in &tool_names[1..] {
        if name == run_name {
            run_len += 1;
        } else {
            flush(run_name, run_len, &mut episodes);
            run_name = name;
            run_len = 1;
        }
    }
    flush(run_name, run_len, &mut episodes);
    episodes
}

/// Load the persisted buffer, append `episodes`, save back. Fail-open:
/// any I/O or parse error keeps the old file (or nothing) and reports
/// `Ok(0)`. Returns the number of episodes appended.
pub fn append_episodes(episodes: &[Episode]) -> usize {
    if episodes.is_empty() {
        return 0;
    }
    let path = match buffer_path() {
        Some(p) => p,
        None => return 0,
    };
    let mut buffer = load_buffer(&path).unwrap_or_default();
    for episode in episodes {
        buffer.record(episode.clone());
    }
    if save_buffer(&path, &buffer).is_err() {
        return 0;
    }
    episodes.len()
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
    fn tool_run_of_three_proposes_procedure() {
        let names = vec![
            "read".to_string(),
            "read".to_string(),
            "read".to_string(),
        ];
        let got = scan_tool_runs(&names, "s1", project());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, RuleKind::Procedure);
    }

    #[test]
    fn tool_run_of_two_proposes_nothing() {
        let names = vec!["read".to_string(), "read".to_string()];
        let got = scan_tool_runs(&names, "s1", project());
        assert!(got.is_empty());
    }

    #[test]
    fn mixed_tool_sequence_finds_only_real_runs() {
        let names = vec![
            "read".to_string(),
            "bash".to_string(),
            "bash".to_string(),
            "bash".to_string(),
            "read".to_string(),
        ];
        let got = scan_tool_runs(&names, "s1", project());
        assert_eq!(got.len(), 1);
        assert!(got[0].trigger.contains("bash"));
    }

    #[test]
    fn empty_inputs_propose_nothing() {
        assert!(scan_user_text(&[], "s1", project()).is_empty());
        assert!(scan_tool_runs(&[], "s1", project()).is_empty());
    }
}
