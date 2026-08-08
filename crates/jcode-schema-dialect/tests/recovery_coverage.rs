//! Does the recovery layer actually cover the providers that need it?
//!
//! The live experiment that closed this loop exercised exactly one route
//! (Antigravity + Gemini). That proves the mechanism works, not that it is
//! *wired* everywhere it is needed, and the recovery code was written before any
//! live loop existed to check it. This runs the same question over every
//! provider dialect at once so a route without recovery is a failing test
//! instead of a discovery during an outage.

use jcode_schema_dialect::{RecoveryAction, quirks, registry};

/// Providers whose runtime calls `recover_from_error`, so a construct the
/// provider rejects is learned and retried instead of failing the turn.
///
/// Kept as data next to the assertion below rather than as prose in a commit
/// message: adding a dialect without recovery should require deliberately
/// editing this list, which is the moment to ask whether that route can 400 on
/// a schema.
const DIALECTS_WITH_RUNTIME_RECOVERY: &[&str] = &[
    "gemini",
    "antigravity-claude",
    "antigravity-bridge",
    "openai",
];

/// Dialects deliberately without runtime recovery, and why.
///
/// OpenAI-family routes reject schemas via a *validation* error naming the
/// construct, which prevention already strips, and their historical failures
/// (#446, #543, #687, #711, #713) were all fixed by not claiming `strict`
/// rather than by retrying. Wiring recovery there is still worthwhile, but it
/// is a separate change with its own live verification, so it is recorded as a
/// known gap instead of being silently absent.
const DIALECTS_WITHOUT_RUNTIME_RECOVERY: &[&str] = &["openrouter", "anthropic"];

#[test]
fn every_dialect_is_accounted_for_as_having_recovery_or_not() {
    let mut registered: Vec<&str> = registry::ALL.iter().map(|spec| spec.id).collect();
    registered.sort_unstable();

    let mut accounted: Vec<&str> = DIALECTS_WITH_RUNTIME_RECOVERY
        .iter()
        .chain(DIALECTS_WITHOUT_RUNTIME_RECOVERY)
        .copied()
        .collect();
    accounted.sort_unstable();

    assert_eq!(
        registered, accounted,
        "a dialect was added or removed without deciding whether its runtime \
         recovers from a schema rejection"
    );
}

/// The classifier must produce an actionable, non-looping recovery for every
/// dialect that has runtime recovery wired.
///
/// A dialect whose `recover_from_error` returns `NotSchemaRelated` for a real
/// rejection would make the retry dead code on that route, which is exactly the
/// failure the single-route live test could not see.
#[test]
fn each_recovering_dialect_learns_retries_once_then_refuses_to_loop() {
    for id in DIALECTS_WITH_RUNTIME_RECOVERY {
        let spec = registry::by_id(id).expect("registered dialect");
        // Isolate per dialect so one case cannot consume another's "new
        // information" signal.
        let dir = tempfile::tempdir().expect("tempdir");
        quirks::use_test_path(dir.path().join(format!("{id}.json")));

        // The real shape these routes return, from issue #754.
        let rejection = format!(
            "{} generateContent failed (HTTP 400 Bad Request): {{\"error\":{{\"code\":400,\
             \"message\":\"Invalid JSON payload received. Unknown name \\\"vendorThing\\\" at \
             'request.tools[0].function_declarations[3].parameters.properties[0].value': Cannot \
             find field.\",\"status\":\"INVALID_ARGUMENT\"}}}}",
            id
        );

        match jcode_schema_dialect::recover_from_error(&rejection, spec) {
            RecoveryAction::RetryWithoutConstruct { description } => assert!(
                description.contains("vendorThing"),
                "dialect `{id}` must name what it learned: {description}"
            ),
            other => panic!("dialect `{id}` cannot recover from a real rejection: {other:?}"),
        }

        // Learned, so normalization now strips it without a request.
        assert!(
            quirks::learned_for(id)
                .rejected_keywords
                .iter()
                .any(|k| k == "vendorThing"),
            "dialect `{id}` did not persist what it learned"
        );

        // And the same rejection again must not retry forever.
        assert!(
            matches!(
                jcode_schema_dialect::recover_from_error(&rejection, spec),
                RecoveryAction::Unrecoverable { .. }
            ),
            "dialect `{id}` would retry the same construct in a loop"
        );
    }
}

/// Recovery must never absorb a rejection that names something load-bearing.
///
/// Stripping `type` or `properties` to make a request succeed would leave the
/// model calling a tool whose real shape it was never told, which is worse than
/// the 400 it replaced. Checked for every dialect, recovering or not.
#[test]
fn no_dialect_absorbs_a_load_bearing_rejection() {
    for spec in registry::ALL {
        let dir = tempfile::tempdir().expect("tempdir");
        quirks::use_test_path(dir.path().join(format!("{}-lb.json", spec.id)));

        let rejection = "GenerateContentRequest.tools[0].function_declarations[3].parameters: \
                         required fields ['label'] are not defined in the schema properties";
        assert!(
            matches!(
                jcode_schema_dialect::recover_from_error(rejection, spec),
                RecoveryAction::Unrecoverable { .. }
            ),
            "dialect `{}` would strip a load-bearing keyword to force a success",
            spec.id
        );
    }
}

/// A transient provider failure must not be mistaken for a schema problem, or
/// recovery would swallow a rate limit and retry it as if a keyword were at
/// fault. Checked for every dialect.
#[test]
fn no_dialect_treats_an_operational_failure_as_a_schema_rejection() {
    let operational = [
        "HTTP 429 Too Many Requests",
        "HTTP 503 upstream unavailable",
        "connection reset by peer",
        "Function call is missing a thought_signature in functionCall parts",
        "Antigravity generateContent failed (HTTP 400 Bad Request): Request contains an invalid argument",
    ];
    for spec in registry::ALL {
        for message in operational {
            assert_eq!(
                jcode_schema_dialect::recover_from_error(message, spec),
                RecoveryAction::NotSchemaRelated,
                "dialect `{}` misread an operational failure as a schema rejection: {message}",
                spec.id
            );
        }
    }
}

/// Runtimes that own a provider request path, and whether each is expected to
/// recover from a schema rejection.
///
/// Per-runtime rather than per-dialect, because dialects are shared: the
/// Antigravity runtime dispatches to the `gemini` dialect too, so a
/// dialect-keyed check stays green even when the *native* Gemini route loses its
/// recovery. That flaw was found by unwiring the native route and watching an
/// earlier version of this test pass anyway.
const RUNTIME_RECOVERY_EXPECTATIONS: &[(&str, bool)] = &[
    ("../jcode-provider-gemini-runtime/src/lib.rs", true),
    ("../jcode-provider-antigravity-runtime/src/lib.rs", true),
    // Learns without retrying: this path owns its own retry and backoff, so a
    // second retry inside it would double attempts against a possibly
    // rate-limited endpoint. Learning still turns "every request fails until a
    // release" into "one request fails".
    (
        "../jcode-provider-openai-runtime/src/openai_provider_impl.rs",
        true,
    ),
    // Still unhandled. Both forward to upstreams whose rejection texts jcode has
    // never captured, so there is nothing to write a classifier against yet;
    // inventing patterns would produce a check that cannot fail. Prevention
    // covers them (their wire output is pinned by
    // `every_provider_sends_clean_schemas`), so this is a real but bounded gap.
    ("../jcode-provider-openrouter-runtime/src/lib.rs", false),
    ("../jcode-provider-anthropic-runtime/src/lib.rs", false),
];

/// Every runtime's recovery wiring must match what this file claims.
///
/// A hand-kept list of "routes that recover" goes stale the first time someone
/// adds or refactors a provider, and its staleness is invisible: the classifier
/// tests keep passing because they never touch the wiring. Reading the runtime
/// sources ties the claim to the code.
#[test]
fn each_runtime_recovery_wiring_matches_what_is_claimed() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut mismatches: Vec<String> = Vec::new();

    for (relative, should_recover) in RUNTIME_RECOVERY_EXPECTATIONS {
        let path = manifest.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read runtime source {}: {err}", path.display()));

        // Count only real calls, not the mentions in doc comments explaining
        // them, or an unwired runtime whose comment survived would pass.
        let calls = source
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                // Either mechanism counts: `recover_from_error` retries the
                // turn, `learn_from_error` only records for the next one. Both
                // end the "fails until a release" behavior, which is what this
                // check is about.
                !trimmed.starts_with("//")
                    && (trimmed.contains("recover_from_error(")
                        || trimmed.contains("learn_from_error("))
            })
            .count();

        let recovers = calls > 0;
        if recovers != *should_recover {
            mismatches.push(format!(
                "{relative}: claimed recovery={should_recover}, found {calls} call(s)"
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "runtime recovery wiring disagrees with this file's claims:\n{}",
        mismatches.join("\n")
    );
}
