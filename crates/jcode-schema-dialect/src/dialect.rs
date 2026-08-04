//! Provider schema dialects, expressed as data.
//!
//! # Why this exists
//!
//! jcode advertises the *whole* tool array on every request, so one construct a
//! provider dislikes does not degrade one tool, it 400s every turn and the
//! provider becomes unusable. That has now happened at least eight times, each
//! with a different keyword and each fixed the same way: append the keyword to
//! that provider's hand-written deny-list and ship a release.
//!
//! | Issue | Provider | Construct |
//! |-------|----------|-----------|
//! | #446  | LM Studio | object schema without `properties` |
//! | #495  | OpenRouter | top-level `anyOf` |
//! | #543  | OpenAI | `format: "uri"` |
//! | #655  | Gemini | `required` naming an undeclared property |
//! | #687  | OpenAI | `uniqueItems` |
//! | #713  | OpenAI | property with no `type` |
//! | #754  | Gemini | `propertyNames` |
//!
//! A deny-list can only ever list what has already broken for somebody, so the
//! next unlisted keyword from the next MCP server is the next outage. The two
//! changes here break that cycle:
//!
//! 1. **Allow-lists, not deny-lists.** A dialect declares what its provider is
//!    known to accept. Anything else is dropped, so an unknown construct is
//!    inert by default rather than fatal by default.
//! 2. **One recursion.** Every dialect shares [`apply`], which walks schemas
//!    using [`crate::keyword`] classification. Adding provider knowledge is a
//!    data change, and a fix to the walk fixes it for all providers at once.

use crate::keyword::{KeywordRole, LOAD_BEARING_KEYWORDS, is_droppable, keyword_role};
use serde_json::{Map, Value};

/// Structural rewrites a dialect can request. These are the transformations
/// that cannot be expressed as "keep this keyword", because they change shape
/// rather than remove a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialectTransforms {
    /// Collapse `anyOf`/`oneOf`/`allOf` at the schema root into one object
    /// whose properties union every branch (Anthropic, OpenRouter: #495).
    pub flatten_top_level_combiners: bool,
    /// Collapse every combiner anywhere to its first branch (the Antigravity
    /// Gemini->Anthropic bridge, which rejects them at any depth).
    pub flatten_all_combiners: bool,
    /// Insert `properties: {}` into object-typed schemas that lack it, which
    /// strict validators require (LM Studio: #446).
    pub require_properties_on_objects: bool,
    /// Drop `required` entries naming a property the same object does not
    /// declare, which Gemini rejects outright (#655).
    pub prune_dangling_required: bool,
    /// Rewrite `const: X` as `enum: [X]` for dialects that model only `enum`.
    pub const_as_enum: bool,
    /// Rewrite `oneOf` as `anyOf` for dialects that model only `anyOf`.
    pub one_of_as_any_of: bool,
    /// Merge each `allOf` branch's `properties` into the enclosing object and
    /// drop the `allOf`.
    ///
    /// Distinct from [`Self::flatten_top_level_combiners`], which widens
    /// `anyOf`/`oneOf` alternatives at the root only. `allOf` branches all
    /// apply simultaneously, so merging them is lossless, and it applies at any
    /// depth. Needed by validators that accept `allOf` syntactically without
    /// intersecting it.
    pub merge_all_of_branches: bool,
}

/// A provider's accepted JSON Schema subset plus the rewrites needed to reach
/// it. Construct these as `const` tables next to the provider they describe.
#[derive(Clone, Debug)]
pub struct DialectSpec {
    /// Stable id used for quirk persistence and diagnostics, e.g. `"gemini"`.
    pub id: &'static str,
    /// Keywords the provider is known to accept. Everything else is dropped.
    pub supported_keywords: &'static [&'static str],
    /// String `format` values the provider accepts. Empty means "any format".
    pub supported_string_formats: &'static [&'static str],
    /// Structural rewrites.
    pub transforms: DialectTransforms,
}

/// Keywords every dialect gets for free: the load-bearing set plus the
/// annotations no provider has ever objected to.
const UNIVERSAL_KEYWORDS: &[&str] = &["type", "properties", "items", "required", "enum", "title"];

impl DialectSpec {
    /// Whether this dialect keeps `key`.
    pub fn supports(&self, key: &str) -> bool {
        UNIVERSAL_KEYWORDS.contains(&key) || self.supported_keywords.contains(&key)
    }

    /// Whether a `format` value survives.
    pub fn supports_format(&self, format: &str) -> bool {
        self.supported_string_formats.is_empty() || self.supported_string_formats.contains(&format)
    }

    /// Check the spec is coherent. A dialect that forgets a load-bearing
    /// keyword would silently strip meaning from every tool, so this is
    /// asserted for all registered dialects by the crate's own tests rather
    /// than left to be discovered in production.
    pub fn validate(&self) -> Result<(), String> {
        for key in LOAD_BEARING_KEYWORDS {
            if !self.supports(key) {
                return Err(format!(
                    "dialect `{}` does not support load-bearing keyword `{key}`",
                    self.id
                ));
            }
        }
        for key in self.supported_keywords {
            if UNIVERSAL_KEYWORDS.contains(key) {
                return Err(format!(
                    "dialect `{}` redundantly lists universal keyword `{key}`",
                    self.id
                ));
            }
        }
        Ok(())
    }
}

/// Extra keywords to drop beyond the dialect's own allow-list, learned at
/// runtime from a real provider rejection (see [`crate::quirks`]).
#[derive(Clone, Debug, Default)]
pub struct LearnedQuirks {
    pub rejected_keywords: Vec<String>,
    pub rejected_formats: Vec<String>,
}

impl LearnedQuirks {
    pub fn is_empty(&self) -> bool {
        self.rejected_keywords.is_empty() && self.rejected_formats.is_empty()
    }
}

/// Normalize a tool-parameter schema into `spec`'s dialect.
pub fn apply(schema: &Value, spec: &DialectSpec) -> Value {
    apply_with_quirks(schema, spec, &LearnedQuirks::default())
}

/// [`apply`], additionally honoring keywords learned from live rejections.
pub fn apply_with_quirks(schema: &Value, spec: &DialectSpec, quirks: &LearnedQuirks) -> Value {
    let mut out = walk(schema, spec, quirks);

    if spec.transforms.flatten_all_combiners {
        out = flatten_all_combiners(&out);
    } else if spec.transforms.flatten_top_level_combiners {
        flatten_top_level_combiners(&mut out);
    }

    // A tool with no parameters is `{}` or a non-object in some MCP servers;
    // strict validators want an object schema (#446).
    if spec.transforms.require_properties_on_objects {
        match out.as_object_mut() {
            Some(map) if map.is_empty() => {
                map.insert("type".into(), Value::String("object".into()));
                map.insert("properties".into(), Value::Object(Map::new()));
            }
            None => {
                out = serde_json::json!({ "type": "object", "properties": {} });
            }
            _ => {}
        }
    }

    out
}

fn walk(schema: &Value, spec: &DialectSpec, quirks: &LearnedQuirks) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                // Renames happen before the support check, because the dialect
                // supports the *renamed* keyword, not the source one. Checking
                // first would drop `oneOf` (and with it an entire branch set)
                // on the very dialects whose whole point is to accept it as
                // `anyOf`.
                let out_key = if key == "oneOf" && spec.transforms.one_of_as_any_of {
                    "anyOf"
                } else {
                    key.as_str()
                };
                let dropped_by_quirk = quirks.rejected_keywords.iter().any(|k| k == key);
                // Load-bearing keywords survive even a learned rejection: if a
                // provider truly rejects `type`, dropping it would produce a
                // meaningless tool, and the right answer is a real fix.
                if (!spec.supports(out_key) || dropped_by_quirk) && is_droppable(key) {
                    if key == "const" && spec.transforms.const_as_enum {
                        out.insert(
                            "enum".into(),
                            // A `const` value is data, so it moves verbatim.
                            Value::Array(vec![value.clone()]),
                        );
                    }
                    continue;
                }
                if key == "format" {
                    let ok = value
                        .as_str()
                        .map(|f| {
                            spec.supports_format(f)
                                && !quirks.rejected_formats.iter().any(|r| r == f)
                        })
                        .unwrap_or(false);
                    if !ok {
                        continue;
                    }
                    out.insert(key.clone(), value.clone());
                    continue;
                }

                let normalized = match keyword_role(key) {
                    KeywordRole::SubschemaMap => match value {
                        Value::Object(children) => Value::Object(
                            children
                                .iter()
                                .map(|(name, child)| (name.clone(), walk(child, spec, quirks)))
                                .collect(),
                        ),
                        other => other.clone(),
                    },
                    KeywordRole::SubschemaArray => match value {
                        Value::Array(items) => {
                            Value::Array(items.iter().map(|i| walk(i, spec, quirks)).collect())
                        }
                        other => walk(other, spec, quirks),
                    },
                    KeywordRole::Subschema => walk(value, spec, quirks),
                    KeywordRole::Data => value.clone(),
                };
                match out.get_mut(out_key) {
                    // `oneOf` renamed onto an existing `anyOf`: merge branches
                    // rather than clobbering one of them.
                    Some(Value::Array(existing)) => {
                        if let Value::Array(incoming) = normalized {
                            existing.extend(incoming);
                        }
                    }
                    _ => {
                        out.insert(out_key.to_string(), normalized);
                    }
                }
            }

            if spec.transforms.prune_dangling_required {
                prune_dangling_required(&mut out);
            }
            if spec.transforms.merge_all_of_branches {
                merge_all_of_branches(&mut out);
            }
            if spec.transforms.require_properties_on_objects && is_object_typed(&out) {
                out.entry("properties".to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|i| walk(i, spec, quirks)).collect()),
        _ => schema.clone(),
    }
}

fn is_object_typed(map: &Map<String, Value>) -> bool {
    match map.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
        _ => false,
    }
}

/// Drop `required` names the same object does not declare (#655).
fn prune_dangling_required(out: &mut Map<String, Value>) {
    let Some(Value::Object(properties)) = out.get("properties") else {
        return;
    };
    let defined: Vec<String> = properties.keys().cloned().collect();
    if let Some(Value::Array(required)) = out.get_mut("required") {
        required.retain(|name| {
            name.as_str()
                .is_some_and(|name| defined.iter().any(|known| known == name))
        });
        if required.is_empty() {
            out.remove("required");
        }
    }
}

/// Collapse root combiners into one object schema whose properties union every
/// branch. Runtime tool deserialization stays the authority on which
/// combination is actually valid.
fn flatten_top_level_combiners(schema: &mut Value) {
    let Some(output) = schema.as_object_mut() else {
        return;
    };
    let mut merged_properties = output
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut all_of_required: Vec<String> = Vec::new();
    let mut saw_combiner = false;

    for keyword in ["oneOf", "anyOf", "allOf"] {
        let Some(branches) = output.remove(keyword).and_then(|v| match v {
            Value::Array(items) => Some(items),
            _ => None,
        }) else {
            continue;
        };
        saw_combiner = true;
        for branch in branches {
            let Some(branch) = branch.as_object() else {
                continue;
            };
            if let Some(properties) = branch.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    merged_properties
                        .entry(name.clone())
                        .or_insert_with(|| property.clone());
                }
            }
            // Only `allOf` branches all apply, so only their `required` is safe
            // to promote; promoting an `anyOf` branch's would demand fields the
            // caller may legitimately omit.
            if keyword == "allOf"
                && let Some(required) = branch.get("required").and_then(Value::as_array)
            {
                for name in required.iter().filter_map(Value::as_str) {
                    if !all_of_required.iter().any(|e| e == name) {
                        all_of_required.push(name.to_string());
                    }
                }
            }
        }
    }

    if !saw_combiner {
        return;
    }

    output.insert("type".into(), Value::String("object".into()));
    output.insert("properties".into(), Value::Object(merged_properties));
    if !all_of_required.is_empty() {
        let required = output
            .entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(required) = required {
            for name in all_of_required {
                if !required.iter().any(|e| e.as_str() == Some(&name)) {
                    required.push(Value::String(name));
                }
            }
        }
    }
    // The union object may now require a name only one branch declared.
    if let Value::Object(map) = schema {
        prune_dangling_required(map);
    }
}

/// Combine two declarations of the same property name, keeping every key
/// either side declares. Neither is strictly more authoritative: the branch
/// narrows the type, the parent carries the description, and a tool needs both.
fn merge_property(parent: &Value, branch: &Value) -> Value {
    let (Some(parent_map), Some(branch_map)) = (parent.as_object(), branch.as_object()) else {
        return parent.clone();
    };
    let mut merged = branch_map.clone();
    for (key, value) in parent_map {
        merged.entry(key.clone()).or_insert_with(|| value.clone());
    }
    Value::Object(merged)
}

/// Collapse every combiner, at any depth, into the schema that contains it.
///
/// The branch is chosen (first, since branches are ordered by primacy) but its
/// *siblings are merged*, not discarded. Merging matters most for `properties`:
/// a multi-action tool declares its shared parameters on the parent object and
/// only its discriminator constraints in the branches, so taking the branch's
/// property map alone would silently drop nearly every parameter the tool
/// accepts. That was live for the `swarm` tool on Antigravity's Claude and
/// bridge routes until the registry-wide conformance sweep caught it.
fn flatten_all_combiners(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            for combiner in ["anyOf", "oneOf", "allOf"] {
                if let Some(Value::Array(branches)) = map.get(combiner)
                    && let Some(first) = branches.first()
                {
                    let mut flattened = match flatten_all_combiners(first) {
                        Value::Object(branch) => branch,
                        other => return other,
                    };
                    for (key, value) in map {
                        if key == combiner {
                            continue;
                        }
                        let sibling = flatten_all_combiners(value);
                        match (flattened.get_mut(key), &sibling) {
                            // Union the parent's declared properties with the
                            // branch's. The parent wins on a name collision:
                            // a branch usually redeclares a shared property
                            // only to narrow it (an `enum` on a discriminator)
                            // and omits the description, so preferring the
                            // branch would delete prompt-visible text.
                            (Some(Value::Object(existing)), Value::Object(incoming))
                                if key == "properties" =>
                            {
                                for (name, property) in incoming {
                                    match existing.get(name) {
                                        Some(parent) => {
                                            existing.insert(
                                                name.clone(),
                                                merge_property(parent, property),
                                            );
                                        }
                                        None => {
                                            existing.insert(name.clone(), property.clone());
                                        }
                                    }
                                }
                            }
                            (Some(_), _) => {}
                            (None, _) => {
                                flattened.insert(key.clone(), sibling);
                            }
                        }
                    }
                    // A branch may require a discriminator the merged object
                    // does not declare, which the downstream validator rejects.
                    prune_dangling_required(&mut flattened);
                    return Value::Object(flattened);
                }
            }
            Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), flatten_all_combiners(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(flatten_all_combiners).collect()),
        _ => schema.clone(),
    }
}

/// Merge each `allOf` branch into the enclosing object and drop the `allOf`.
///
/// Lossless in principle: `allOf` branches all apply at once, so their
/// properties and requirements are simply the object's. Needed by validators
/// that accept `allOf` syntactically without intersecting it, which otherwise
/// see a tool with no properties at all.
///
/// Only object-shaped branches are merged. A branch expressing something else
/// (a bare `{"minLength": 1}` on a string, say) has nothing to contribute to a
/// property map, and dropping it only widens what the model may send while the
/// tool still validates the real call.
fn merge_all_of_branches(out: &mut Map<String, Value>) {
    let Some(Value::Array(branches)) = out.remove("allOf") else {
        return;
    };

    let mut properties = out
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut required: Vec<String> = out
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    for branch in &branches {
        let Some(branch) = branch.as_object() else {
            continue;
        };
        // Adopt the branch's `type` when the enclosing object declares none.
        // A schema whose only `type` lives in its `allOf` branches becomes
        // typeless once the `allOf` is gone, and a typeless tool schema is
        // rejected outright.
        if !out.contains_key("type")
            && let Some(branch_type) = branch.get("type")
        {
            out.insert("type".to_string(), branch_type.clone());
        }
        if let Some(branch_properties) = branch.get("properties").and_then(Value::as_object) {
            for (name, property) in branch_properties {
                // The enclosing object's own declaration wins: it is the more
                // specific one, and a branch usually only narrows.
                properties
                    .entry(name.clone())
                    .or_insert_with(|| property.clone());
            }
        }
        if let Some(branch_required) = branch.get("required").and_then(Value::as_array) {
            for name in branch_required.iter().filter_map(Value::as_str) {
                if !required.iter().any(|existing| existing == name) {
                    required.push(name.to_string());
                }
            }
        }
    }

    if !properties.is_empty() {
        out.insert("properties".to_string(), Value::Object(properties));
    }
    if required.is_empty() {
        out.remove("required");
    } else {
        out.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    // A branch may have required a name no branch declared.
    prune_dangling_required(out);
}
