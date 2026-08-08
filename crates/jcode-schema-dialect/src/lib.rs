//! Provider JSON Schema compatibility, as a system rather than a patch queue.
//!
//! # The recurring bug
//!
//! jcode sends its entire tool array on every request. Providers each accept a
//! different subset of JSON Schema, so a single construct emitted by one MCP
//! server does not degrade one tool: it 400s every turn and takes the provider
//! offline. Issues #446, #495, #543, #655, #687, #711, #713 and #754 are all
//! this same bug with a different keyword, and each was fixed by appending that
//! keyword to a hand-written per-provider deny-list.
//!
//! A deny-list only contains what has already broken for a user, so that loop
//! cannot converge. This crate replaces it with three layers:
//!
//! 1. **Prevention** ([`registry`], [`dialect`]) - each provider declares the
//!    subset it *accepts*. Unknown constructs are dropped instead of forwarded,
//!    so a keyword nobody has seen yet is inert rather than fatal.
//! 2. **Recovery** ([`rejection`]) - when a provider rejects something anyway,
//!    its error text is parsed into the offending keyword so the turn can be
//!    retried without it, instead of failing in front of the user.
//! 3. **Memory** ([`quirks`]) - the learned rejection is persisted, so it costs
//!    one wasted round trip ever, not one per request, and the fix propagates
//!    without waiting for a jcode release.
//!
//! # Adding a provider
//!
//! Add a [`dialect::DialectSpec`] to [`registry`] listing the keywords it is
//! observed to accept, then call [`normalize`] where tools are built. Do not
//! write a new recursion: [`dialect::apply`] handles keyword classification,
//! and a bug fixed there is fixed for every provider at once.

pub mod conformance;
pub mod dialect;
pub mod keyword;
pub mod quirks;
pub mod registry;
pub mod rejection;

pub use conformance::{
    ConformanceError, must_not_contain_unsupported_constructs, must_preserve_meaning,
    untyped_properties,
};
pub use dialect::{DialectSpec, DialectTransforms, LearnedQuirks};
pub use keyword::{KeywordRole, keyword_role};
pub use rejection::{SchemaRejection, classify, is_schema_error};

use serde_json::Value;

/// Normalize a tool-parameter schema for `spec`, applying anything already
/// learned about that provider. This is the entry point provider crates use.
pub fn normalize(schema: &Value, spec: &DialectSpec) -> Value {
    let learned = quirks::learned_for(spec.id);
    if learned.is_empty() {
        dialect::apply(schema, spec)
    } else {
        dialect::apply_with_quirks(schema, spec, &learned)
    }
}

/// What a caller should do after a provider rejected a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Not a schema problem; handle the error normally.
    NotSchemaRelated,
    /// A schema rejection naming something new. The tool schemas have been
    /// re-normalized to exclude it, so retrying the request should succeed.
    RetryWithoutConstruct {
        /// Human-readable description for the log line.
        description: String,
    },
    /// A schema rejection that retrying cannot fix, either because it names
    /// something jcode already strips or because it names nothing actionable.
    /// The message explains what to add to the dialect.
    Unrecoverable { hint: String },
}

/// Classify a provider failure and, when it is a learnable schema rejection,
/// record it so the next [`normalize`] call strips it.
///
/// Returns [`RecoveryAction::RetryWithoutConstruct`] at most once per distinct
/// construct per dialect, which is what bounds the retry loop: a second
/// rejection naming the same keyword reports as unrecoverable instead of
/// retrying forever.
pub fn recover_from_error(message: &str, spec: &DialectSpec) -> RecoveryAction {
    let Some(rejection) = rejection::classify(message) else {
        if rejection::is_schema_error(message) {
            return RecoveryAction::Unrecoverable {
                hint: format!(
                    "provider `{}` rejected the tool schemas but did not name the construct; \
                     the raw error is the only diagnostic available",
                    spec.id
                ),
            };
        }
        return RecoveryAction::NotSchemaRelated;
    };

    let mut learned_something = false;
    let mut described = Vec::new();

    // Learn every keyword the response named, not just the first. A Gemini 400
    // reports one `fieldViolations` entry per bad keyword, so learning them one
    // per turn would burn a failed request for each (observed live: a single
    // response naming both `dependentRequired` and `unevaluatedItems`).
    for keyword in &rejection.keywords {
        let keyword = keyword.as_str();
        // A load-bearing keyword cannot be dropped without destroying the
        // tool's meaning, so this is a real bug rather than a quirk to absorb.
        if !keyword::is_droppable(keyword) {
            return RecoveryAction::Unrecoverable {
                hint: format!(
                    "provider `{}` rejected the load-bearing keyword `{keyword}`, which cannot be \
                     stripped without changing what the tool accepts; the dialect's transforms \
                     need a real fix",
                    spec.id
                ),
            };
        }
        if quirks::record_keyword(spec.id, keyword) {
            learned_something = true;
            described.push(format!("keyword `{keyword}`"));
        }
    }
    if let Some(format) = rejection.format.as_deref()
        && quirks::record_format(spec.id, format)
    {
        learned_something = true;
        described.push(format!("format `{format}`"));
    }

    if learned_something {
        let tool = rejection
            .tool
            .as_deref()
            .map(|t| format!(" (from tool `{t}`)"))
            .unwrap_or_default();
        return RecoveryAction::RetryWithoutConstruct {
            description: format!(
                "provider `{}` rejected {}{tool}; stripping it and retrying, and remembering it \
                 for future requests",
                spec.id,
                described.join(" and "),
            ),
        };
    }

    RecoveryAction::Unrecoverable {
        hint: if rejection.is_actionable() {
            format!(
                "provider `{}` rejected {} again after it was already being stripped, so the \
                 construct is not what actually failed",
                spec.id,
                rejection
                    .keyword()
                    .map(ToString::to_string)
                    .or(rejection.format)
                    .unwrap_or_else(|| "a schema construct".into())
            )
        } else {
            format!(
                "provider `{}` reported a schema error naming no actionable construct",
                spec.id
            )
        },
    }
}

/// Learn from a rejection without retrying, returning a message to show the
/// user in place of the raw provider error.
///
/// Some request paths cannot safely re-send a turn: OpenAI's streaming loop
/// owns its own retry and backoff machinery, and threading a second retry
/// through it risks doubling attempts against a rate-limited endpoint. But the
/// expensive half of recovery is *learning*, not retrying. Recording the
/// construct here means the user's next request already omits it, so a schema
/// rejection costs one failed turn instead of every turn until a release.
///
/// Returns `None` when the error is not about tool schemas, so callers can use
/// this as a pass-through predicate.
pub fn learn_from_error(message: &str, spec: &DialectSpec) -> Option<String> {
    match recover_from_error(message, spec) {
        RecoveryAction::NotSchemaRelated => None,
        RecoveryAction::RetryWithoutConstruct { description } => Some(format!(
            "{description}. This request failed, but the next one will not send it.",
        )),
        RecoveryAction::Unrecoverable { hint } => Some(hint),
    }
}

#[cfg(test)]
mod learn_tests {
    use super::*;

    fn isolated(name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        quirks::use_test_path(dir.path().join(format!("{name}.json")));
        dir
    }

    /// The two OpenAI rejections that name a construct must be learned, so the
    /// user's next request omits it even though this path cannot retry.
    #[test]
    fn openai_rejections_that_name_a_construct_are_learned() {
        let _dir = isolated("learn-openai");

        let format_rejection = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__firecrawl__firecrawl_map': In context=('properties', 'url'), 'uri' is not a valid format.";
        let message = learn_from_error(format_rejection, &registry::OPENAI).expect("recognized");
        assert!(message.contains("uri"), "{message}");
        assert!(
            quirks::learned_for("openai")
                .rejected_formats
                .iter()
                .any(|f| f == "uri"),
            "the format must be remembered for the next request"
        );

        let keyword_rejection = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__x__y': In context=('properties', 'ids'), 'uniqueItems' is not permitted.";
        let message = learn_from_error(keyword_rejection, &registry::OPENAI).expect("recognized");
        assert!(message.contains("uniqueItems"), "{message}");
    }

    /// #713's "must have a 'type' key" names nothing strippable, so it must be
    /// labelled as a schema problem rather than either retried or passed
    /// through as an opaque 400.
    #[test]
    fn a_structural_openai_rejection_is_labelled_not_retried() {
        let _dir = isolated("learn-structural");
        let structural = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__cua__set_config': In context=('properties','value'), schema must have a 'type' key.";
        let message = learn_from_error(structural, &registry::OPENAI).expect("recognized");
        assert!(
            message.contains("schema"),
            "the user needs to know this is a tool-schema problem: {message}"
        );
        assert!(
            quirks::learned_for("openai").is_empty(),
            "nothing is strippable here, so nothing must be recorded"
        );
    }

    #[test]
    fn operational_failures_pass_straight_through() {
        let _dir = isolated("learn-operational");
        for message in [
            "HTTP 429 Too Many Requests",
            "connection reset by peer",
            "HTTP 500 internal error",
        ] {
            assert!(
                learn_from_error(message, &registry::OPENAI).is_none(),
                "misread an operational failure as a schema problem: {message}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Redirect this test thread's quirk store so parallel cases cannot see
    /// each other's learned rejections.
    fn isolate_quirks(name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        quirks::use_test_path(dir.path().join(format!("{name}.json")));
        dir
    }

    /// The exact schema from #754 (`@playwright/mcp`'s `browser_drop`) must
    /// survive the Gemini dialect with the offending keyword gone and the rest
    /// of the tool intact.
    #[test]
    fn issue_754_playwright_schema_is_accepted_by_the_gemini_dialect() {
        let schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "propertyNames": { "type": "string" },
                    "description": "Data to drop, as a map of MIME type to string value"
                }
            },
            "required": ["data"]
        });
        let out = dialect::apply(&schema, &registry::GEMINI);
        assert!(!rejection::schema_contains_keyword(&out, "propertyNames"));
        assert!(!rejection::schema_contains_keyword(
            &out,
            "additionalProperties"
        ));
        assert_eq!(out["properties"]["data"]["type"], "object");
        assert_eq!(
            out["properties"]["data"]["description"],
            "Data to drop, as a map of MIME type to string value"
        );
        assert_eq!(out["required"], json!(["data"]));
    }

    /// #687: `uniqueItems` is stripped for OpenAI but the property itself, its
    /// type, and its description survive.
    #[test]
    fn issue_687_unique_items_is_stripped_without_losing_the_property() {
        let schema = json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "uniqueItems": true,
                    "items": { "type": "string" },
                    "description": "Channel ids"
                }
            }
        });
        let out = dialect::apply(&schema, &registry::OPENAI);
        assert!(out["properties"]["ids"].get("uniqueItems").is_none());
        assert_eq!(out["properties"]["ids"]["items"]["type"], "string");
        assert_eq!(out["properties"]["ids"]["description"], "Channel ids");
    }

    /// #543: an unsupported `format` goes, a supported one stays.
    #[test]
    fn issue_543_unsupported_format_is_stripped_but_supported_ones_survive() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "format": "uri" },
                "when": { "type": "string", "format": "date-time" }
            }
        });
        let out = dialect::apply(&schema, &registry::OPENAI);
        assert!(out["properties"]["url"].get("format").is_none());
        assert_eq!(out["properties"]["url"]["type"], "string");
        assert_eq!(out["properties"]["when"]["format"], "date-time");
    }

    /// #446: a bare no-argument object schema gains `properties`.
    #[test]
    fn issue_446_bare_object_schema_gains_properties() {
        let out = dialect::apply(&json!({ "type": "object" }), &registry::OPENROUTER);
        assert_eq!(out["properties"], json!({}));
        let empty = dialect::apply(&json!({}), &registry::OPENROUTER);
        assert_eq!(empty["type"], "object");
        assert_eq!(empty["properties"], json!({}));
    }

    /// #495: a top-level combiner is flattened into one object for providers
    /// whose upstream rejects it.
    #[test]
    fn issue_495_top_level_combiner_is_flattened() {
        let schema = json!({
            "type": "object",
            "properties": { "action": { "type": "string" } },
            "anyOf": [
                { "properties": { "label": { "type": "string" } }, "required": ["label"] },
                { "properties": { "target": { "type": "string" } } }
            ]
        });
        let out = dialect::apply(&schema, &registry::OPENROUTER);
        assert!(out.get("anyOf").is_none());
        assert_eq!(out["properties"]["action"]["type"], "string");
        assert_eq!(out["properties"]["label"]["type"], "string");
        assert_eq!(out["properties"]["target"]["type"], "string");
    }

    /// #655: `required` naming a property this object does not declare is
    /// dropped for Gemini, since Gemini 400s on it.
    #[test]
    fn issue_655_dangling_required_is_pruned_for_gemini() {
        let schema = json!({
            "type": "object",
            "properties": { "action": { "type": "string" } },
            "required": ["action", "label"]
        });
        let out = dialect::apply(&schema, &registry::GEMINI);
        assert_eq!(out["required"], json!(["action"]));
    }

    /// The regression that made #754 a bug in the first place: a top-level-only
    /// filter left the nested copy in place.
    #[test]
    fn stripping_reaches_arbitrarily_deep_nesting() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "array", "items": {
                    "type": "object", "properties": {
                        "b": { "type": "object", "propertyNames": { "type": "string" } }
                    }
                }}
            }
        });
        let out = dialect::apply(&schema, &registry::GEMINI);
        assert!(!rejection::schema_contains_keyword(&out, "propertyNames"));
    }

    /// #713's sibling hazard: a *property* named like a keyword must not be
    /// mistaken for the keyword and deleted.
    #[test]
    fn a_property_named_like_a_keyword_is_never_stripped() {
        let schema = json!({
            "type": "object",
            "properties": {
                "uniqueItems": { "type": "boolean", "description": "a real field" },
                "propertyNames": { "type": "string" }
            }
        });
        let openai = dialect::apply(&schema, &registry::OPENAI);
        assert_eq!(openai["properties"]["uniqueItems"]["type"], "boolean");
        let gemini = dialect::apply(&schema, &registry::GEMINI);
        assert_eq!(gemini["properties"]["propertyNames"]["type"], "string");
    }

    /// Enum values are data, so a value that happens to name a keyword must
    /// survive verbatim.
    #[test]
    fn enum_values_that_look_like_keywords_survive() {
        let schema = json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["uniqueItems", "propertyNames"] }
            }
        });
        let out = dialect::apply(&schema, &registry::GEMINI);
        assert_eq!(
            out["properties"]["mode"]["enum"],
            json!(["uniqueItems", "propertyNames"])
        );
    }

    #[test]
    fn recovery_learns_then_refuses_to_loop() {
        let _dir = isolate_quirks("recover");
        let message = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__x__y': In context=('properties', 'ids'), 'somethingNew' is not permitted.";

        match recover_from_error(message, &registry::OPENAI) {
            RecoveryAction::RetryWithoutConstruct { description } => {
                assert!(description.contains("somethingNew"), "{description}");
            }
            other => panic!("expected a retry, got {other:?}"),
        }

        // The same rejection a second time means stripping it did not help, so
        // the caller must not retry again.
        assert!(matches!(
            recover_from_error(message, &registry::OPENAI),
            RecoveryAction::Unrecoverable { .. }
        ));
    }

    #[test]
    fn a_learned_keyword_is_stripped_by_later_normalization() {
        let _dir = isolate_quirks("learned");
        let schema = json!({
            "type": "object",
            "properties": { "ids": { "type": "array", "vendorThing": true } }
        });
        // `vendorThing` is unknown to the dialect, so prevention already drops
        // it; use a keyword the dialect explicitly supports to prove the
        // learned layer adds power beyond the allow-list.
        assert!(rejection::schema_contains_keyword(
            &json!({ "minItems": 1 }),
            "minItems"
        ));
        let supported = json!({
            "type": "object",
            "properties": { "ids": { "type": "array", "minItems": 1 } }
        });
        assert!(rejection::schema_contains_keyword(
            &normalize(&supported, &registry::OPENAI),
            "minItems"
        ));

        assert!(quirks::record_keyword("openai", "minItems"));
        assert!(!rejection::schema_contains_keyword(
            &normalize(&supported, &registry::OPENAI),
            "minItems"
        ));
        let _ = schema;
    }

    #[test]
    fn a_load_bearing_rejection_is_reported_not_absorbed() {
        let _dir = isolate_quirks("loadbearing");
        let action = recover_from_error(
            "GenerateContentRequest.tools[0].function_declarations[3].parameters: required fields ['label'] are not defined in the schema properties",
            &registry::GEMINI,
        );
        match action {
            RecoveryAction::Unrecoverable { hint } => assert!(hint.contains("required"), "{hint}"),
            other => panic!("required must never be silently dropped, got {other:?}"),
        }
    }

    #[test]
    fn non_schema_errors_are_passed_through() {
        assert_eq!(
            recover_from_error("HTTP 503 upstream unavailable", &registry::OPENAI),
            RecoveryAction::NotSchemaRelated
        );
    }

    /// Found by the registry-wide conformance sweep, not by a user: flattening
    /// a combiner used to keep only the chosen branch's `properties`, so the
    /// `swarm` tool reached Antigravity's Claude route advertising 3 of its 44
    /// parameters. Requests still succeeded, which is why nothing caught it.
    #[test]
    fn flattening_a_combiner_keeps_the_parent_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "what to do" },
                "target": { "type": "string", "description": "on what" },
                "limit": { "type": "integer" }
            },
            "required": ["action"],
            "anyOf": [
                {
                    "properties": { "action": { "enum": ["spawn"] } },
                    "required": ["action", "prompt"]
                },
                { "properties": { "action": { "enum": ["stop"] } } }
            ]
        });

        let out = dialect::apply(&schema, &registry::ANTIGRAVITY_CLAUDE);

        // Every parameter the tool actually accepts is still advertised.
        for name in ["action", "target", "limit"] {
            assert!(
                out["properties"].get(name).is_some(),
                "property `{name}` was dropped: {out}"
            );
        }
        // The branch's narrowing survives alongside the parent's description.
        assert_eq!(out["properties"]["action"]["enum"], json!(["spawn"]));
        assert_eq!(out["properties"]["action"]["description"], "what to do");
        assert_eq!(out["properties"]["target"]["description"], "on what");
        // No combiner survives for a route that rejects them at any depth.
        assert!(out.get("anyOf").is_none());
        // And the branch's `required` cannot name a property that is now absent.
        assert_eq!(out["required"], json!(["action"]));
    }

    /// A keyword the dialect accepts only under a different name must be
    /// renamed, never dropped. Getting the order wrong deleted the entire
    /// `oneOf` branch set (and with it the `batch` tool's whole call shape)
    /// before it could be renamed to `anyOf`.
    #[test]
    fn a_renamed_keyword_survives_instead_of_being_dropped() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            { "type": "object", "properties": {
                                "tool": { "type": "string", "const": "read" }
                            }}
                        ]
                    }
                }
            }
        });
        let out = dialect::apply(&schema, &registry::GEMINI);
        let items = &out["properties"]["tool_calls"]["items"];
        assert!(items.get("oneOf").is_none(), "oneOf must be renamed");
        assert_eq!(
            items["anyOf"][0]["properties"]["tool"]["enum"],
            json!(["read"]),
            "the branch and its const->enum rewrite must both survive: {out}"
        );
    }

    /// The same sweep's second finding: a branch that redeclares a shared
    /// property to narrow it omits the description, so preferring the branch
    /// wholesale deleted prompt-visible text from the tool.
    #[test]
    fn a_narrowing_branch_never_deletes_the_parent_description() {
        let schema = json!({
            "type": "object",
            "properties": { "label": { "type": "string", "description": "shown on the chip" } },
            "anyOf": [{ "properties": { "label": { "minLength": 1 } } }]
        });
        for spec in [&registry::ANTIGRAVITY_CLAUDE, &registry::ANTIGRAVITY_BRIDGE] {
            let out = dialect::apply(&schema, spec);
            assert_eq!(
                out["properties"]["label"]["description"], "shown on the chip",
                "dialect `{}` dropped the description",
                spec.id
            );
            assert_eq!(out["properties"]["label"]["type"], "string");
        }
    }
}
