//! Detects ChatGPT web answers that claim jcode's tools are unavailable.
//!
//! ChatGPT's own prior ("I can't access your computer") sometimes wins over
//! the jcode protocol. The caller re-asks once with a correction when an
//! answer without tool calls makes such a claim.
//!
//! The rules come from labeled real answers. A sentence counts when it holds a
//! negation and one of three shapes:
//! - first person, an access verb and a generic capability ("I can't run
//!   commands on your computer");
//! - a scope naming this chat ("in this session", "from this environment")
//!   plus a tool or access word ("I couldn't reconnect to the tool transport
//!   in this session");
//! - Jcode or local-computer tools as the subject ("the Jcode tools aren't
//!   connected").
//!
//! Quoted material (code, quotes, block quotes) is ignored, so commit
//! subjects or jcode's own notes that mention such claims never match.

const NEGATIONS: &[&str] = &[
    "can't",
    "cannot",
    "can not",
    "couldn't",
    "could not",
    "unable to",
    "not able to",
];
/// Negations that already mean "no access" or "not available".
const NO_ACCESS: &[&str] = &[
    "don't have access",
    "do not have access",
    "have no access",
    "no access",
    "lack access",
    "don't have the ability",
    "do not have the ability",
    "don't have a",
    "do not have a",
    "aren't connected",
    "are not connected",
    "isn't connected",
    "is not connected",
    "aren't available",
    "are not available",
    "are unavailable",
    "is unavailable",
    "isn't available",
    "is not available",
    "aren't accessible",
    "are not accessible",
    "aren't exposed",
    "are not exposed",
    "aren't enabled",
    "are not enabled",
    "don't work",
    "do not work",
    "not connected",
];
const VERBS: &[&str] = &[
    "use",
    "run",
    "execute",
    "call",
    "invoke",
    "access",
    "reach",
    "interact",
    "operate",
    "control",
    "touch",
    "modify",
    "edit",
    "connect",
    "reconnect",
    "resume",
];
/// Capabilities whose absence is a refusal whoever claims it.
const CAPABILITIES: &[&str] = &[
    "jcode tool",
    "jcode's tool",
    "jcode local",
    "jcode's local",
    " tools",
    "tool call",
    "local computer",
    "local-computer",
    "local machine",
    "local files",
    "local filesystem",
    "local file system",
    "local shell",
    "local environment",
    "local execution",
    "your computer",
    "your machine",
    "your files",
    "your filesystem",
    "your file system",
    "your shell",
    "your terminal",
    "the user's computer",
    "the user's machine",
    " commands",
    " a shell",
];
/// Phrases that place the claim in this chat rather than in a tool result.
const SCOPES: &[&str] = &[
    "this session",
    "this chat",
    "this conversation",
    "this environment",
    "this thread",
];
/// Weaker capability words, only trusted together with a scope.
const SCOPED_CAPABILITIES: &[&str] = &[" tool", "access", "shell", "connect", "computer"];
const TOOL_SUBJECTS: &[&str] = &[
    "jcode tools",
    "jcode's tools",
    "jcode local-computer tools",
    "jcode's local-computer tools",
    "local-computer tools",
    "local computer tools",
    "jcode tool interface",
    "jcode's tool interface",
    "jcode shell",
    "jcode's shell",
];

/// Whether a final answer (one without tool envelopes) claims the model
/// cannot use Jcode's tools or reach the user's computer. Ordinary failures
/// ("I can't find the config", "the build cannot run on Windows") don't match.
pub(super) fn claims_no_tool_access(text: &str) -> bool {
    let normalized = text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    strip_quoted(&normalized)
        .split(['.', '!', '?', '\n', ';', ':'])
        .any(|sentence| {
            let words: Vec<&str> = sentence.split_whitespace().collect();
            sentence_claims_no_access(&format!(" {} ", words.join(" ")))
        })
}

fn sentence_claims_no_access(sentence: &str) -> bool {
    let contains_any = |list: &[&str], hay: &str| list.iter().any(|item| hay.contains(item));
    // The first negation, as (start, end, means no access by itself).
    let negation = NEGATIONS
        .iter()
        .map(|neg| (neg, false))
        .chain(NO_ACCESS.iter().map(|neg| (neg, true)))
        .filter_map(|(neg, no_access)| {
            let at = sentence.find(&format!(" {neg}"))? + 1;
            Some((at, at + neg.len(), no_access))
        })
        .min_by_key(|&(start, end, _)| (start, std::cmp::Reverse(end)));
    let Some((start, end, no_access)) = negation else {
        return false;
    };
    // Bind the predicate to its subject, not an earlier tool mention in a
    // contrast clause ("Jcode tools work, GitHub isn't connected").
    let lead = sentence[..start].trim_end();
    let subject = [" here", " currently", " still", " now"]
        .iter()
        .find_map(|suffix| lead.strip_suffix(*suffix))
        .unwrap_or(lead);
    if TOOL_SUBJECTS.iter().any(|tool| {
        subject
            .strip_suffix(*tool)
            .is_some_and(|before| before.is_empty() || before.ends_with(char::is_whitespace))
    }) {
        return true;
    }
    let first_person = [" i", " i'm", " i've", " we", " i am", " we are"]
        .iter()
        .any(|person| subject.ends_with(*person));
    let after = &sentence[end..];
    // Scoped claim: "I couldn't reach the tool transport in this session", or
    // a list of tools that "aren't connected in this session".
    let clause = lead
        .rsplit(',')
        .next()
        .unwrap_or(lead)
        .rsplit(" but ")
        .next()
        .unwrap_or(lead);
    let tool_subject = clause.trim_end().ends_with(" tools");
    if contains_any(SCOPES, sentence)
        && (first_person || tool_subject)
        && (contains_any(CAPABILITIES, after) || contains_any(SCOPED_CAPABILITIES, after))
    {
        return true;
    }
    // The access verb must follow the negation closely ("can't directly run"),
    // unless the negation itself means "no access".
    let verb_follows = no_access
        || after
            .split_whitespace()
            .take(2)
            .any(|word| VERBS.contains(&word));
    first_person && verb_follows && contains_any(CAPABILITIES, after)
}

/// Drops code fences, inline code, double-quoted spans and block-quote lines.
fn strip_quoted(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.trim_start().starts_with('>') {
            continue;
        }
        let mut closer = None;
        for ch in line.chars() {
            match (closer, ch) {
                (Some(close), _) if ch == close => closer = None,
                (Some(_), _) => {}
                (None, '`') => closer = Some('`'),
                (None, '"') => closer = Some('"'),
                (None, '\u{201c}') => closer = Some('\u{201d}'),
                (None, _) => kept.push(ch),
            }
        }
        kept.push('\n');
    }
    kept
}
/// Whether a final answer is only a promise to act ("I'll check the machine
/// directly.") with no tool call and no result. Observed live from GPT-5.6
/// Sol: the turn ended there, so the user got no answer. Only a short,
/// single-sentence answer that starts with a first-person intention counts.
pub(super) fn only_announces_action(text: &str) -> bool {
    let normalized = text.trim().to_lowercase().replace('\u{2019}', "'");
    let starts = [
        "i'll ",
        "i will ",
        "let me ",
        "i'm going to ",
        "i am going to ",
    ];
    let sentences = normalized
        .split(['.', '!', '?', '\n'])
        .filter(|sentence| !sentence.trim().is_empty())
        .count();
    normalized.len() <= 160
        && sentences == 1
        && !normalized.ends_with('?')
        && starts.iter().any(|start| normalized.starts_with(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_claims_that_jcode_tools_are_unavailable() {
        // Verbatim from the failing gpt-6[web] session.
        let observed = "I’ll work through all five PRs.\n\nI can continue the review and plan, but I can’t execute Jcode’s local-computer tools from this session, so I can’t safely claim any fixes or pushes.";
        assert!(claims_no_tool_access(observed));
        // Verbatim from the second failing turn: an impersonal claim.
        let impersonal = "The Jcode local-computer tools aren’t connected to this chat’s executable tool interface, so I haven’t made further changes or pushes. The next step is to resume this session through Jcode’s connected interface, preserving the existing worktrees and main checkout.";
        assert!(claims_no_tool_access(impersonal));
        for claim in [
            "The Jcode tools are unavailable in this session.",
            "Jcode's tools aren't available here, so nothing was run.",
            "Local-computer tools are not exposed to this conversation.",
            "I cannot run commands on your computer.",
            "I don't have access to your files from here.",
            "I'm unable to use the tools in this chat.",
            "Unfortunately I can't directly access your local machine",
            "I do not have access to the user's computer",
            "I am not able to execute shell commands",
            // Scoped claims seen in real answers, paraphrased.
            "I can give a summary, but I can’t access `src/main.rs` from this environment. Please paste it.",
            "I couldn't reconnect to the Jcode tool transport in this session, so nothing changed.",
            "I couldn't find a local Git tool in this session's available integrations.",
            "I couldn't resume: the Jcode shell, editor and Git tools aren't connected as callable tools in this session.",
            "Next, restore the Jcode tool connection. I don't have a shell in this chat.",
            "We can't use the tools in this conversation.",
        ] {
            assert!(claims_no_tool_access(claim), "missed: {claim}");
        }
    }

    #[test]
    fn ignores_ordinary_failures_and_third_party_limits() {
        for text in [
            "I can't find /tmp/x: it does not exist.",
            "The build cannot run locally on Windows without the MSVC tools.",
            "I couldn't reproduce the failure, so the test may be flaky.",
            "Users cannot use the tools menu until the dialog closes.",
            "I cannot claim the fix works yet: the tests still fail.",
            "Done. All 166 tests pass and the branch is pushed.",
            "The CI job can't access the cache, so the toolchain reinstalls each run.",
            "I can't edit the PR description without your approval.",
            "The Jcode tools work, but GitHub's services are unavailable",
            "The Jcode tools work, GitHub isn't connected.",
            "The Jcode tools work, GitHub isn't connected in this session.",
            "The Jcode tools work but GitHub isn't connected in this session.",
            "The Jcode tools work but GitHub isn't connected.",
            "The tools menu is unavailable until the dialog closes.",
            "GitHub's API is not available right now, so the push failed.",
            "The Jcode tools returned an error: permission denied on /etc/hosts.",
            // Quoted or reported claims are not the model's own.
            "```text\nabc123 fix: catch \"jcode tools aren't connected\" refusals\n```\nExit code 0.",
            "Commit: `fix: stop models claiming jcode tools are unavailable`.",
            "> Note: ChatGPT used its own sandbox, not this computer.\n\nDone.",
            "The check catches sentences where the Jcode tools are said to be not connected.",
            // Limits in this session that are not about tool access.
            "The tests cannot run in this environment without network access.",
            "I can't responsibly mark it ready to push: CI is not verified.",
            "I couldn't complete a fresh live check of the remote heads in this session.",
            "I haven't completed the changes, so I can't report a commit SHA.",
            "The changes are not committed, so they cannot have been pushed.",
        ] {
            assert!(!claims_no_tool_access(text), "false positive: {text}");
        }
    }

    #[test]
    fn a_bare_promise_to_act_is_detected_but_real_answers_are_not() {
        // Observed live from GPT-5.6 Sol, the whole final answer.
        for text in [
            "I’ll check the machine directly.",
            "I’ll check the machine’s memory directly.",
            "Let me look at the disk usage.",
            "I'm going to run the tests now.",
        ] {
            assert!(only_announces_action(text), "missed: {text}");
        }
        for text in [
            "I’ll check the machine directly. It has 8 logical CPUs.",
            "The hostname is `HIQ-1`.",
            "I'll need the file path to continue?",
            "I will not push without CI.\n\nThe branch is ready for review.",
            "SWEEP_OK",
        ] {
            assert!(!only_announces_action(text), "false positive: {text}");
        }
    }
}
