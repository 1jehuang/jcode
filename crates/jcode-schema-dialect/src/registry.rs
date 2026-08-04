//! The dialect table: what each provider is known to accept.
//!
//! Every entry is evidence-based. A keyword is listed as supported because
//! jcode has observed the provider accept it in a live request, not because the
//! spec says it should. That asymmetry is deliberate: the cost of omitting a
//! keyword the provider would have accepted is a slightly less expressive tool
//! schema, while the cost of listing one it rejects is every request failing.

use crate::dialect::{DialectSpec, DialectTransforms};

/// Annotations and validation keywords the OpenAI function-parameters subset
/// accepts. Derived from the deny-list this replaces (#543, #687, #711, #713)
/// inverted against the keywords jcode and common MCP servers actually emit.
pub const OPENAI: DialectSpec = DialectSpec {
    id: "openai",
    supported_keywords: &[
        "description",
        "default",
        "examples",
        "format",
        "anyOf",
        "oneOf",
        "allOf",
        "$defs",
        "definitions",
        "$ref",
        "$schema",
        "const",
        "additionalProperties",
        "patternProperties",
        "prefixItems",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "nullable",
    ],
    // #543: unknown formats such as `uri` fail the strict validator.
    supported_string_formats: &[
        "date-time",
        "time",
        "date",
        "duration",
        "email",
        "hostname",
        "ipv4",
        "ipv6",
        "uuid",
    ],
    transforms: DialectTransforms {
        one_of_as_any_of: true,
        // OpenAI accepts `allOf` but does not intersect its branches the way
        // the spec requires, so a schema whose properties live only in `allOf`
        // branches ends up advertising none of them. The sanitizer this
        // replaces merged them into the parent object, so the dialect must too
        // or migrating onto it would silently empty those tools (caught by
        // diffing the two before switching).
        merge_all_of_branches: true,
        ..DEFAULT_TRANSFORMS
    },
};

/// Gemini `generateContent` accepts an OpenAPI 3.0 subset for
/// `function_declarations.parameters`, which is much narrower than OpenAI's:
/// draft keywords like `$ref`/`$defs`/`additionalProperties`/`propertyNames`
/// are rejected with HTTP 400 (#754), and it validates `required` against the
/// same object's `properties` (#655).
pub const GEMINI: DialectSpec = DialectSpec {
    id: "gemini",
    supported_keywords: &[
        "description",
        "default",
        "format",
        "anyOf",
        "nullable",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "pattern",
        "example",
    ],
    supported_string_formats: &[],
    transforms: DialectTransforms {
        prune_dangling_required: true,
        const_as_enum: true,
        one_of_as_any_of: true,
        ..DEFAULT_TRANSFORMS
    },
};

/// Anthropic accepts full draft 2020-12 inside properties but rejects
/// combiners at the input schema's top level.
pub const ANTHROPIC: DialectSpec = DialectSpec {
    id: "anthropic",
    supported_keywords: &[
        "description",
        "default",
        "examples",
        "format",
        "anyOf",
        "oneOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "$defs",
        "definitions",
        "$ref",
        "$schema",
        "$comment",
        "const",
        "additionalProperties",
        "patternProperties",
        "propertyNames",
        "prefixItems",
        "contains",
        "uniqueItems",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
        "dependentRequired",
        "dependentSchemas",
        "nullable",
    ],
    supported_string_formats: &[],
    transforms: DialectTransforms {
        flatten_top_level_combiners: true,
        // Anthropic's `input_schema` must be an object schema with a
        // `properties` map: the API rejects a bare `{"type":"object"}` and a
        // non-object schema outright. The provider's own sanitizer guaranteed
        // both, so the dialect has to as well or migrating onto it would be a
        // regression (caught by diffing the two before switching).
        require_properties_on_objects: true,
        ..DEFAULT_TRANSFORMS
    },
};

/// OpenRouter forwards to whichever upstream serves the model, so it must
/// satisfy the strictest of them: Anthropic-family top-level combiner
/// rejection (#495) plus LM Studio's demand for `properties` (#446).
pub const OPENROUTER: DialectSpec = DialectSpec {
    id: "openrouter",
    supported_keywords: OPENAI.supported_keywords,
    supported_string_formats: &[],
    transforms: DialectTransforms {
        flatten_top_level_combiners: true,
        require_properties_on_objects: true,
        ..DEFAULT_TRANSFORMS
    },
};

/// Antigravity's Claude route: the Cloud Code backend translates the request
/// to Anthropic *after* parsing it.
///
/// The keyword set is Gemini's, not Anthropic's, because every Antigravity
/// request is a `generateContent` payload whatever model it names. A keyword
/// outside the Gemini schema proto is rejected while parsing the payload
/// ("Invalid JSON payload received. Unknown name ...", #754), so it never
/// reaches the Anthropic translation that would have accepted it. On top of
/// that, the translation rejects combiners at any depth.
pub const ANTIGRAVITY_CLAUDE: DialectSpec = DialectSpec {
    id: "antigravity-claude",
    supported_keywords: GEMINI.supported_keywords,
    supported_string_formats: &[],
    transforms: DialectTransforms {
        flatten_all_combiners: true,
        prune_dangling_required: true,
        const_as_enum: true,
        one_of_as_any_of: true,
        ..DEFAULT_TRANSFORMS
    },
};

/// Antigravity's OpenAI-compatible bridge (gpt-oss and friends).
///
/// Gemini's payload subset again (see [`ANTIGRAVITY_CLAUDE`]), minus the
/// numeric bounds this bridge corrupts: it round-trips them through a protobuf
/// `int64`, which proto3 JSON re-encodes as a string, and then rejects the
/// string it just produced ("'10' is not of type 'integer'"). The bounds are
/// advisory, so dropping them costs nothing at call time.
pub const ANTIGRAVITY_BRIDGE: DialectSpec = DialectSpec {
    id: "antigravity-bridge",
    supported_keywords: &[
        "description",
        "default",
        "format",
        "anyOf",
        "nullable",
        "minimum",
        "maximum",
        "pattern",
        "example",
    ],
    supported_string_formats: &[],
    transforms: DialectTransforms {
        flatten_all_combiners: true,
        prune_dangling_required: true,
        const_as_enum: true,
        one_of_as_any_of: true,
        ..DEFAULT_TRANSFORMS
    },
};

const DEFAULT_TRANSFORMS: DialectTransforms = DialectTransforms {
    flatten_top_level_combiners: false,
    flatten_all_combiners: false,
    require_properties_on_objects: false,
    prune_dangling_required: false,
    const_as_enum: false,
    one_of_as_any_of: false,
    merge_all_of_branches: false,
};

/// Every registered dialect, for conformance sweeps.
pub const ALL: &[&DialectSpec] = &[
    &OPENAI,
    &GEMINI,
    &ANTHROPIC,
    &OPENROUTER,
    &ANTIGRAVITY_CLAUDE,
    &ANTIGRAVITY_BRIDGE,
];

/// Look up a dialect by its stable id (used by the quirk store and CLI).
pub fn by_id(id: &str) -> Option<&'static DialectSpec> {
    ALL.iter().copied().find(|spec| spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_dialect_is_coherent() {
        for spec in ALL {
            spec.validate().unwrap_or_else(|err| panic!("{err}"));
        }
    }

    #[test]
    fn dialect_ids_are_unique() {
        let mut ids: Vec<&str> = ALL.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate dialect id");
    }

    #[test]
    fn lookup_round_trips() {
        for spec in ALL {
            assert_eq!(by_id(spec.id).map(|s| s.id), Some(spec.id));
        }
        assert!(by_id("nope").is_none());
    }
}
