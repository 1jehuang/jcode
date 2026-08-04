//! JSON Schema keyword classification.
//!
//! Every provider-specific schema rewrite in jcode has to answer the same two
//! questions about a key it encounters: *is this a schema keyword or a property
//! name?* and *does its value contain more schemas?* Getting either wrong is how
//! a property literally named `uniqueItems` gets stripped, or how a nested
//! `propertyNames` survives a top-level filter (issue #754).
//!
//! Answering them once, here, is what lets the rest of this crate express a
//! provider dialect as data instead of as another bespoke recursion.

/// Where a keyword's value sits on the "is it a schema?" spectrum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeywordRole {
    /// The value is one subschema (`items`, `not`, `propertyNames`). It may also
    /// legally be a boolean, which recursion passes through untouched.
    Subschema,
    /// The value is a map of name -> subschema (`properties`, `$defs`). The map
    /// keys are user-controlled names, never keywords.
    SubschemaMap,
    /// The value is an array of subschemas (`anyOf`, `prefixItems`).
    SubschemaArray,
    /// The value is plain data that must never be walked as a schema (`enum`,
    /// `const`, `required`, `default`, `examples`).
    Data,
}

/// Keywords whose value is a map of name -> subschema.
const SUBSCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// Keywords whose value is a single subschema (or a boolean schema).
const SUBSCHEMA_KEYWORDS: &[&str] = &[
    "items",
    "additionalItems",
    "additionalProperties",
    "contains",
    "propertyNames",
    "not",
    "if",
    "then",
    "else",
    "unevaluatedItems",
    "unevaluatedProperties",
    "contentSchema",
];

/// Keywords whose value is an array of subschemas.
const SUBSCHEMA_ARRAY_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Keywords whose value is data, not a schema. Recursing into these is how a
/// user's `enum: ["items"]` or `default: {"type": "x"}` gets corrupted.
const DATA_KEYWORDS: &[&str] = &["enum", "const", "default", "examples", "required"];

/// Classify a schema keyword by what its value contains.
pub fn keyword_role(key: &str) -> KeywordRole {
    if SUBSCHEMA_MAP_KEYWORDS.contains(&key) {
        KeywordRole::SubschemaMap
    } else if SUBSCHEMA_KEYWORDS.contains(&key) {
        KeywordRole::Subschema
    } else if SUBSCHEMA_ARRAY_KEYWORDS.contains(&key) {
        KeywordRole::SubschemaArray
    } else if DATA_KEYWORDS.contains(&key) {
        KeywordRole::Data
    } else {
        // Unknown keywords (vendor extensions, future drafts) are treated as
        // data: walking into them can only corrupt them, and a dialect that
        // does not support them drops them wholesale anyway.
        KeywordRole::Data
    }
}

/// Keywords that carry the *meaning* of a schema rather than an extra
/// constraint or annotation on it.
///
/// A dialect may drop any keyword it does not support, which is the whole point
/// of the allow-list: a construct jcode has never seen cannot brick a provider.
/// But dropping one of these would silently change what the tool accepts, so
/// they are never droppable, and a dialect that omits one is a bug caught by
/// [`crate::dialect::DialectSpec::validate`].
pub const LOAD_BEARING_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "items",
    "required",
    "enum",
    "description",
];

/// Whether a keyword may be removed from a schema without changing which
/// instances the tool will accept at execution time.
///
/// Removing a validation keyword (`uniqueItems`, `minItems`, `propertyNames`)
/// only widens what the *model* is told it may send; the tool or MCP server
/// still validates the real call, so the constraint is not actually lost.
pub fn is_droppable(key: &str) -> bool {
    !LOAD_BEARING_KEYWORDS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_bearing_keywords_are_classified() {
        assert_eq!(keyword_role("properties"), KeywordRole::SubschemaMap);
        assert_eq!(keyword_role("propertyNames"), KeywordRole::Subschema);
        assert_eq!(keyword_role("anyOf"), KeywordRole::SubschemaArray);
        assert_eq!(keyword_role("enum"), KeywordRole::Data);
    }

    #[test]
    fn unknown_keywords_are_data_so_recursion_cannot_corrupt_them() {
        assert_eq!(keyword_role("x-vendor-thing"), KeywordRole::Data);
        assert_eq!(keyword_role("$anchor"), KeywordRole::Data);
    }

    #[test]
    fn load_bearing_keywords_are_not_droppable() {
        for key in LOAD_BEARING_KEYWORDS {
            assert!(!is_droppable(key), "{key} must never be dropped");
        }
        assert!(is_droppable("uniqueItems"));
        assert!(is_droppable("propertyNames"));
    }
}
