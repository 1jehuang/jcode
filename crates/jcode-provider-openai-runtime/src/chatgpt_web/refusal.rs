//! Detects ChatGPT web answers that claim jcode's tools are unavailable.
//!
//! ChatGPT's own prior ("I can't access your computer") sometimes wins over
//! the jcode protocol. The caller re-asks once with a correction when an
//! answer without tool calls makes such a claim.

/// Whether a final answer (one without tool envelopes) claims the model itself
/// cannot use Jcode's tools or reach the user's computer. Only a first-person
/// claim that pairs an access verb with the capability counts, so "I can't
/// find the jcode config" or "the build cannot run locally" are not matches.
pub(super) fn claims_no_tool_access(text: &str) -> bool {
    const SUBJECTS: &[&str] = &["i", "i'm", "i am", "i've", "i have"];
    /// Negations followed by an access verb ("can't run ...").
    const NEGATIONS: &[&str] = &["can't", "cannot", "can not", "unable to", "not able to"];
    /// Negations that already mean "no access".
    const NO_ACCESS: &[&str] = &[
        "don't have access",
        "do not have access",
        "have no access",
        "no access",
        "lack access",
        "not connected",
        "don't have the ability",
        "do not have the ability",
    ];
    const VERBS: &[&str] = &[
        "use", "run", "execute", "call", "invoke", "access", "reach", "interact", "operate",
        "control", "touch", "modify", "edit",
    ];
    const CAPABILITIES: &[&str] = &[
        "jcode tool",
        "jcode's tool",
        "jcode's local",
        "jcode local",
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
    ];
    let normalized = text.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    // Impersonal form: "the Jcode tools aren't connected to this chat",
    // "local-computer tools are unavailable in this session". Only the Jcode
    // or local-computer tools count, so "the tools menu is unavailable" or a
    // third-party service being down never match.
    const TOOL_SUBJECTS: &[&str] = &[
        "jcode tools",
        "jcode's tools",
        "jcode local-computer tools",
        "jcode's local-computer tools",
        "local-computer tools",
        "local computer tools",
        "jcode tool interface",
        "jcode's tool interface",
    ];
    const UNAVAILABLE: &[&str] = &[
        "aren't connected",
        "are not connected",
        "isn't connected",
        "is not connected",
        "aren't available",
        "are not available",
        "are unavailable",
        "is unavailable",
        "aren't accessible",
        "are not accessible",
        "aren't exposed",
        "are not exposed",
        "aren't enabled",
        "are not enabled",
        "don't work",
        "do not work",
    ];
    let impersonal = normalized
        .split(['.', '!', '?', '\n', ';'])
        .any(|sentence| {
            TOOL_SUBJECTS.iter().any(|subject| {
                sentence.find(subject).is_some_and(|at| {
                    let rest = &sentence[at + subject.len()..];
                    UNAVAILABLE.iter().any(|claim| rest.contains(claim))
                })
            })
        });
    if impersonal {
        return true;
    }
    normalized
        .split(['.', '!', '?', '\n', ';', ':'])
        .any(|sentence| {
            let words: Vec<&str> = sentence.split_whitespace().collect();
            let sentence = format!(" {} ", words.join(" "));
            let has_capability = |rest: &str| {
                CAPABILITIES
                    .iter()
                    .any(|capability| rest.contains(capability))
            };
            SUBJECTS.iter().any(|subject| {
                let verb_claim = NEGATIONS.iter().any(|negation| {
                    let claim = format!(" {subject} {negation} ");
                    sentence.find(&claim).is_some_and(|at| {
                        let rest = &sentence[at + claim.len() - 1..];
                        // The access verb must come right after the negation,
                        // allowing one adverb ("can't directly run").
                        let leading: Vec<&str> = rest.split_whitespace().take(2).collect();
                        leading.iter().any(|word| VERBS.contains(word)) && has_capability(rest)
                    })
                });
                verb_claim
                    || NO_ACCESS.iter().any(|negation| {
                        let claim = format!(" {subject} {negation}");
                        sentence
                            .find(&claim)
                            .is_some_and(|at| has_capability(&sentence[at + claim.len()..]))
                    })
            })
        })
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
            "The tools menu is unavailable until the dialog closes.",
            "GitHub's API is not available right now, so the push failed.",
            "The Jcode tools returned an error: permission denied on /etc/hosts.",
        ] {
            assert!(!claims_no_tool_access(text), "false positive: {text}");
        }
    }
}
