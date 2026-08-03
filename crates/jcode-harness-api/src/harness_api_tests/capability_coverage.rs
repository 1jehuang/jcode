//! Capability coverage: what the daemon can do versus what the SDK can reach.
//!
//! The harness API is a curated subset of the internal protocol, and that is
//! correct: 77 internal requests should not become 77 public ones. But
//! "curated" and "missing something important" look identical from inside the
//! API crate, and the gap only shows up when someone tries to build a real
//! client and finds they cannot switch models.
//!
//! So the reference clients are the specification. The TUI and desktop2 are
//! complete, shipping clients of the same daemon; every request they send is
//! by definition something a client needs. This test diffs that set against
//! the API surface and fails when an unreviewed gap appears.
//!
//! The ledger below is the reviewed answer for each one. Adding a request to
//! the TUI without deciding what it means for the API fails this test, which
//! is the point: the decision gets made deliberately, once, and is recorded
//! where the next person will find it.

use std::collections::BTreeSet;

/// Why a daemon capability the reference clients use is not in the harness API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    /// Reachable through the API today.
    Covered,
    /// Deliberately internal: it serves the TUI's own presentation or process
    /// model, and would mean nothing to a third-party client.
    ClientInternal,
    /// A real gap. Worth exposing, not yet done. Every entry needs a reason
    /// that says what a client cannot build without it.
    Gap(&'static str),
}

use Disposition::{ClientInternal, Covered, Gap};

/// Reviewed disposition of every internal request the reference clients send.
///
/// Sorted by name so additions produce clean diffs.
const LEDGER: &[(&str, Disposition)] = &[
    ("Account", ClientInternal),
    ("BackgroundTool", ClientInternal),
    ("Cancel", Covered),
    (
        "CancelSoftInterrupts",
        Gap("a client that can queue soft interrupts cannot take them back"),
    ),
    ("Clear", Covered),
    ("ClientDebugResponse", ClientInternal),
    (
        "Compact",
        Gap(
            "no way to compact a long transcript; a long-running client eventually hits the context limit with no recourse",
        ),
    ),
    ("CycleModel", ClientInternal),
    ("GetCompactedHistory", ClientInternal),
    ("GetHistory", Covered),
    (
        "GetModelCatalog",
        Gap("a client cannot list available models, so it cannot offer a model picker"),
    ),
    ("InputShell", ClientInternal),
    ("Login", ClientInternal),
    ("Message", Covered),
    ("Model", ClientInternal),
    ("NotifyAuthChanged", ClientInternal),
    ("RefreshModels", ClientInternal),
    ("Reload", ClientInternal),
    (
        "RenameSession",
        Gap("session titles are read-only; a session list cannot be curated"),
    ),
    ("ResumeAllSessions", ClientInternal),
    ("ResumeSession", ClientInternal),
    ("Rewind", Covered),
    (
        "RewindUndo",
        Gap("rewind is destructive and cannot be undone through the API"),
    ),
    ("RunSubagent", ClientInternal),
    ("SetCompactionMode", ClientInternal),
    ("SetFeature", ClientInternal),
    (
        "SetModel",
        Gap("a client cannot choose the model for a session, which is table stakes for a chat UI"),
    ),
    ("SetPremiumMode", ClientInternal),
    (
        "SetReasoningEffort",
        Gap("reasoning effort is a per-turn cost/quality dial with no API equivalent"),
    ),
    ("SetRoute", ClientInternal),
    ("SetServiceTier", ClientInternal),
    ("SetSubagentModel", ClientInternal),
    ("SetTransport", ClientInternal),
    ("SoftInterrupt", Covered),
    ("Split", ClientInternal),
    ("StdinResponse", ClientInternal),
    ("Subscribe", Covered),
    ("SwitchAnthropicAccount", ClientInternal),
    ("SwitchOpenAiAccount", ClientInternal),
    ("Transcript", ClientInternal),
    ("Transfer", ClientInternal),
    ("TriggerMemoryExtraction", ClientInternal),
];

/// Requests the reference clients (TUI, desktop2) send to the daemon.
fn reference_client_requests() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for dir in ["../jcode-tui/src", "../jcode-desktop2/src"] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
        collect_requests(&root, &mut found);
    }
    // The desktop2 client speaks the *API*, so its `ApiRequest::` uses are
    // covered by construction and would otherwise pollute the diff.
    for api_only in [
        "CreateSession",
        "AttachSession",
        "ListSessions",
        "PeekSession",
        "SendMessage",
    ] {
        found.remove(api_only);
    }
    found
}

fn collect_requests(dir: &std::path::Path, found: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_requests(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (index, _) in source.match_indices("Request::") {
                let name: String = source[index + "Request::".len()..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric())
                    .collect();
                if name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                {
                    found.insert(name);
                }
            }
        }
    }
}

/// Every capability the reference clients use must have a reviewed entry.
///
/// This is the gate: it does not demand that the API cover everything, only
/// that nothing is missing *by accident*.
#[test]
fn every_reference_client_capability_is_triaged() {
    let used = reference_client_requests();
    if used.is_empty() {
        // Vendored/packaged builds without the sibling crates: nothing to do.
        return;
    }
    let ledger: BTreeSet<&str> = LEDGER.iter().map(|(name, _)| *name).collect();

    let untriaged: Vec<&String> = used
        .iter()
        .filter(|name| !ledger.contains(name.as_str()))
        .collect();
    assert!(
        untriaged.is_empty(),
        "these daemon requests are used by the TUI/desktop but have no reviewed \
         disposition in the harness API capability ledger: {untriaged:?}\n\
         Add each to LEDGER in capability_coverage.rs as Covered, ClientInternal, \
         or Gap(\"what a client cannot build without it\")."
    );

    let stale: Vec<&str> = ledger
        .iter()
        .filter(|name| !used.contains(**name))
        .copied()
        .collect();
    assert!(
        stale.is_empty(),
        "the capability ledger lists requests the reference clients no longer send: \
         {stale:?}. Remove them so the ledger keeps describing reality."
    );
}

/// Print the current gap list. Not a failure: gaps are a roadmap, not a bug.
///
/// Run with `cargo test -p jcode-harness-api -- --nocapture capability_report`.
#[test]
fn capability_report() {
    let covered = LEDGER.iter().filter(|(_, d)| *d == Covered).count();
    let internal = LEDGER.iter().filter(|(_, d)| *d == ClientInternal).count();
    let gaps: Vec<(&str, &str)> = LEDGER
        .iter()
        .filter_map(|(name, disposition)| match disposition {
            Gap(reason) => Some((*name, *reason)),
            _ => None,
        })
        .collect();

    println!("\nharness API capability coverage");
    println!("  covered by the API:      {covered}");
    println!("  deliberately internal:   {internal}");
    println!("  known gaps:              {}", gaps.len());
    for (name, reason) in &gaps {
        println!("    - {name}: {reason}");
    }
    println!();
}
