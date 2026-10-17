//! Build a JSON Schema for a tool and demonstrate `ensure_intent_in_schema`.
//!
//! `jcode_tool_core` exposes the two schema property builders
//! every tool call sends:
//! - `intent_schema_property()` — the short `intent` field the UI
//!   uses to label a tool call
//! - `accept_large_output_schema_property()` — the opt-in flag
//!   the context guard checks before re-running an over-budget tool
//!
//! It also exposes `ensure_intent_in_schema()`, which mutates a
//! tool's existing JSON Schema in place to add the intent field
//! if the tool author forgot it.
//!
//! Run with: `cargo run -p jcode-tool-core --example build_schema`

use jcode_tool_core::{
    ACCEPT_LARGE_OUTPUT_KEY, accept_large_output_schema_property, ensure_intent_in_schema,
    intent_schema_property,
};
use serde_json::json;

fn main() {
    let intent = intent_schema_property();
    let accept = accept_large_output_schema_property();

    println!("intent_schema_property:        {intent}");
    println!("accept_large_output_property:  {accept}");
    println!("ACCEPT_LARGE_OUTPUT_KEY:       {ACCEPT_LARGE_OUTPUT_KEY}");
    println!();

    // A tool author writes their own JSON Schema.
    let schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"]
    });

    ensure_intent_in_schema(schema.clone());

    let updated = ensure_intent_in_schema(schema);
    println!("schema after ensure_intent_in_schema: {updated}");
}
