# jcode DCP Integration Plan

> **Goal**: Integrate DCP as a plugin layer that runs BEFORE jcode CompactionManager.
> **Branch**: `feat/dcp-integration` in `/data/projects/jcode-dcp/`
> **Depends on**: DCP_API_PLAN.md must be completed first.

---

## Architecture

```
Agent::messages_for_provider()
       │
       │  1. Get all messages from session
       │
       ├─ [DCP Plugin Layer] ← NEW
       │    ├─ jcode_to_dcp() convert messages
       │    ├─ pruner.transform_messages_with_diff()
       │    ├─ dcp_to_jcode() convert back
       │    └─ Return TransformResult { messages, tokens_saved, removed_ids }
       │
       ├─ [CompactionManager] ← EXISTING (unchanged)
       │    ├─ Receives DCP-pruned messages (fewer tokens)
       │    ├─ Token budget check (80%/95%)
       │    ├─ LLM summary / OpenAI native compaction
       │    └─ Returns final API payload
       │
       └─ Return messages → Provider API
```

---

## Prerequisites

DCP APIs are now complete (all 4 items implemented). Before building jcode with DCP:

- [x] `dcp_bridge.rs` — jcode ↔ DCP message converter (already exists in jcode)
- [x] `has_pending_work()` — DCP API Plan D1 ✅
- [x] `transform_messages_with_diff()` — DCP API Plan D2 ✅
- [x] `count_messages_tokens()` — DCP API Plan D3 ✅
- [x] `last_nudge_kind()` — DCP API Plan D4 ✅
- [x] DCP committed at `https://github.com/quangdang46/dynamic_context_pruning`

---

## Tasks

### J1: Update Cargo.toml dependency

**File**: `/data/projects/jcode-dcp/Cargo.toml`

```toml
# Use git URL — DCP is published at github.com/quangdang46/dynamic_context_pruning
# The `dynamic_context_pruning` package is the umbrella crate at crates/dynamic_context_pruning/
dynamic_context_pruning = { git = "https://github.com/quangdang46/dynamic_context_pruning", branch = "main", package = "dynamic_context_pruning" }

[features]
dcp = ["dep:dynamic_context_pruning"]
```

> **Dev note**: During active DCP development, you may want to use a local path instead:
> ```toml
> dynamic_context_pruning = { path = "/path/to/your/dynamic_context_pruning/crates/dynamic_context_pruning", optional = true }
> ```
> Switch back to the git URL when DCP features are stable.

Verify:
```bash
cargo check --features dcp
```

---

### J2: Register `mod dcp_bridge` in lib.rs

**File**: `/data/projects/jcode-dcp/src/lib.rs`

Add after existing module declarations:

```rust
#[cfg(feature = "dcp")]
mod dcp_bridge;
```

---

### J3: Create DCP Plugin struct

**File**: `/data/projects/jcode-dcp/src/dcp_plugin.rs` (NEW)

```rust
//! DCP Plugin — wraps ContextPruner and bridges jcode ↔ DCP types.

#[cfg(feature = "dcp")]
use dynamic_context_pruning::{
    Config, ContextPruner, Message as DcpMessage, Part as DcpPart, Role as DcpRole,
    TransformResult,
};
use crate::message::Message as JMsg;
use crate::message::ContentBlock;

/// DCP plugin that wraps ContextPruner and handles type conversion.
pub struct DcpPlugin {
    pruner: ContextPruner,
    enabled: bool,
}

impl DcpPlugin {
    /// Create a new DCP plugin with default config.
    pub fn new() -> Result<Self, String> {
        let config = Config::default();
        let pruner = ContextPruner::new(config)
            .map_err(|e| format!("DCP init failed: {e:?}"))?;
        Ok(Self { pruner, enabled: true })
    }

    /// Create with custom config.
    pub fn with_config(config: Config) -> Result<Self, String> {
        let pruner = ContextPruner::new(config)
            .map_err(|e| format!("DCP init failed: {e:?}"))?;
        Ok(Self { pruner, enabled: true })
    }

    /// Enable/disable DCP at runtime.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Check if DCP has pending work to do.
    pub fn has_pending_work(&self) -> bool {
        self.pruner.has_pending_work()
    }

    /// Run DCP transform on jcode messages.
    ///
    /// Returns the transformed messages and a diff of what changed.
    /// If DCP is disabled, returns the input unchanged with changed=false.
    pub fn transform(
        &mut self,
        messages: &[JMsg],
    ) -> Result<DcpTransformOutput, String> {
        if !self.enabled || messages.is_empty() {
            return Ok(DcpTransformOutput {
                messages: messages.to_vec(),
                tokens_saved: 0,
                removed_count: 0,
                changed: false,
            });
        }

        // 1. Convert jcode → DCP
        let dcp_messages = dcp_bridge::jcode_to_dcp(messages);

        // 2. Run DCP transform with diff
        let result = self.pruner
            .transform_messages_with_diff(dcp_messages)
            .map_err(|e| format!("DCP transform error: {e:?}"))?;

        // 3. Convert DCP → jcode
        let jcode_messages = dcp_bridge::dcp_to_jcode(result.messages);

        Ok(DcpTransformOutput {
            messages: jcode_messages,
            tokens_saved: result.tokens_saved,
            removed_count: result.removed_message_ids.len(),
            changed: result.changed,
        })
    }

    /// Inject DCP system prompt addendum.
    pub fn transform_system(&self, system: &mut String) {
        if self.enabled {
            self.pruner.transform_system(system);
        }
    }

    /// Get cumulative stats.
    pub fn stats(&self) -> &dynamic_context_pruning::Stats {
        self.pruner.stats()
    }

    /// Get the underlying pruner (for tool handling).
    pub fn pruner(&self) -> &ContextPruner {
        &self.pruner
    }

    pub fn pruner_mut(&mut self) -> &mut ContextPruner {
        &mut self.pruner
    }
}

/// Output of a DCP transform pass.
pub struct DcpTransformOutput {
    /// Transformed jcode messages.
    pub messages: Vec<JMsg>,
    /// Estimated tokens saved.
    pub tokens_saved: u64,
    /// Number of messages removed.
    pub removed_count: usize,
    /// Whether any changes were made.
    pub changed: bool,
}
```

---

### J4: Add DCP field to Agent

**File**: `/data/projects/jcode-dcp/src/agent.rs`

Add a field to the `Agent` struct:

```rust
pub struct Agent {
    // ... existing fields ...

    /// DCP plugin for context pruning (behind feature flag).
    #[cfg(feature = "dcp")]
    dcp: Option<DcpPlugin>,
}
```

Initialize in `Agent::new()`:

```rust
#[cfg(feature = "dcp")]
{
    self.dcp = if config.dcp_enabled() {
        Some(DcpPlugin::new()?)
    } else {
        None
    };
}
```

---

### J5: Wire DCP into `messages_for_provider()`

**File**: `/data/projects/jcode-dcp/src/agent.rs` — `messages_for_provider()` method (~line 581)

This is the **main integration point**. Insert DCP call BEFORE compaction:

```rust
fn messages_for_provider(&mut self, provider: Arc<dyn Provider>) -> Vec<Message> {
    let all_messages = self.session.messages();

    // ── Phase 1: DCP Plugin Layer ──────────────────────────────────
    #[cfg(feature = "dcp")]
    let messages = {
        if let Some(dcp) = &mut self.dcp {
            let output = dcp.transform(&all_messages).unwrap_or_else(|e| {
                tracing::warn!("DCP transform failed: {e}");
                DcpTransformOutput {
                    messages: all_messages.clone(),
                    tokens_saved: 0,
                    removed_count: 0,
                    changed: false,
                }
            });

            if output.changed {
                tracing::info!(
                    "DCP: pruned {} messages, saved ~{} tokens",
                    output.removed_count,
                    output.tokens_saved,
                );
            }

            output.messages
        } else {
            all_messages.clone()
        }
    };

    #[cfg(not(feature = "dcp"))]
    let messages = all_messages.clone();

    // ── Phase 2: CompactionManager (existing) ──────────────────────
    if provider.supports_compaction() {
        let mut manager = self.compaction.write().await;
        manager.ensure_context_fits(&messages, provider.clone());
        manager.messages_for_api_with(&messages)
    } else {
        messages
    }
}
```

**Key point**: CompactionManager now receives DCP-pruned messages.
Fewer tokens → compaction triggers less often → better context quality.

---

### J6: Wire DCP system prompt injection

**File**: `/data/projects/jcode-dcp/src/agent.rs`

Find where the system prompt is built (typically `build_system_message()` or similar).

```rust
fn build_system_message(&self, system: &mut String) {
    // ... existing system prompt building ...

    #[cfg(feature = "dcp")]
    if let Some(dcp) = &self.dcp {
        dcp.transform_system(system);
    }
}
```

---

### J7: Register compress tool in jcode tool registry

**File**: `/data/projects/jcode-dcp/src/tool/mod.rs`

Add DCP compress tool registration:

```rust
#[cfg(feature = "dcp")]
{
    if let Some(dcp) = &agent.dcp {
        let schema = dcp.pruner().compress_tool_schema();
        registry.register("compress", Arc::new(DcpCompressTool::new(agent_handle))).await;
    }
}
```

**Create new file**: `/data/projects/jcode-dcp/src/tool/dcp_compress.rs`

```rust
//! DCP compress tool — exposed as a jcode tool for manual compression.

use crate::tool::{Tool, ToolContext, ToolResult};
use dynamic_context_pruning::{CompressArgs, ContextPruner, Message as DcpMessage};

pub struct DcpCompressTool {
    // Handle back to agent for accessing DCP pruner
    agent: AgentHandle,
}

#[async_trait]
impl Tool for DcpCompressTool {
    fn name(&self) -> &str { "compress" }
    fn description(&self) -> &str {
        "Compress conversation ranges or messages into compact summaries to free context"
    }
    fn parameters_schema(&self) -> serde_json::Value { /* from compress_tool_schema() */ }

    async fn execute(&self, input: serde_json::Value, ctx: ToolContext) -> Result<ToolResult, String> {
        // 1. Parse CompressArgs from input
        // 2. Get current messages from session
        // 3. Convert to DCP messages
        // 4. Call pruner.handle_compress(args, &dcp_messages)
        // 5. Format result for display
        // 6. Return ToolResult
    }
}
```

Also register `decompress` and `recompress` tools similarly.

---

### J8: Add `/dcp` slash command

**File**: `/data/projects/jcode-dcp/src/slash_command.rs` (or wherever slash commands are handled)

```rust
#[cfg(feature = "dcp")]
{
    "/dcp" => {
        if let Some(dcp) = &mut agent.dcp {
            let messages = agent.session.messages();
            let dcp_messages = dcp_bridge::jcode_to_dcp(&messages);
            let outcome = dcp.pruner_mut()
                .handle_command(subcmd, &args, &dcp_messages);

            match outcome {
                CommandOutcome::Context { text } => display_text(&text),
                CommandOutcome::Stats { text } => display_text(&text),
                CommandOutcome::Sweep { count } => display_text(&format!("Swept {count} items")),
                CommandOutcome::Manual { enabled } => display_text(&format!(
                    "Manual mode: {}", if enabled { "ON" } else { "OFF" }
                )),
                _ => display_text("Unknown /dcp command"),
            }
        } else {
            display_text("DCP not enabled. Build with --features dcp");
        }
    }
}
```

Supported subcommands:
- `/dcp context` — show session breakdown
- `/dcp stats` — show cumulative statistics
- `/dcp sweep` — trigger manual sweep
- `/dcp manual on|off` — toggle manual mode

---

### J9: Persist DCP state across sessions

**File**: `/data/projects/jcode-dcp/src/session.rs`

Add DCP state to session persistence:

```rust
pub struct Session {
    // ... existing fields ...

    /// DCP state blob (opaque to jcode, managed by DCP).
    #[cfg(feature = "dcp")]
    pub dcp_state: Option<String>, // serialized DCP SessionState
}
```

**On save**: `dcp.pruner().save()` handles this internally via `FileStateStore`.
jcode just needs to ensure `set_session_id()` is called with the jcode session ID.

**On restore**: In `seed_compaction_from_session()`, add:

```rust
#[cfg(feature = "dcp")]
if let Some(dcp) = &mut self.dcp {
    dcp.pruner_mut().set_session_id(&session.id);
}
```

---

### J10: Add DCP config to jcode config

**File**: `/data/projects/jcode-dcp/crates/jcode-config-types/src/lib.rs`

```rust
pub struct FeatureConfig {
    // ... existing fields ...

    /// Enable DCP context pruning plugin.
    #[cfg(feature = "dcp")]
    pub dcp_enabled: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            // ...
            #[cfg(feature = "dcp")]
            dcp_enabled: true,
        }
    }
}
```

Also add to `jcode.toml` config file schema:

```toml
[features]
dcp_enabled = true
```

---

## Implementation Order

```
Phase A: Foundation
  J1 (Update Cargo.toml)           ← verify compiles with --features dcp
  J2 (Register mod dcp_bridge)     ← enable existing bridge
  J3 (Create DcpPlugin struct)     ← wrapper + type conversion
  J4 (Add DCP field to Agent)      ← storage
       ↓
Phase B: Core Integration
  J5 (Wire into messages_for_provider)  ← THE key integration point
  J6 (Wire system prompt injection)     ← DCP system prompt addendum
       ↓
Phase C: Tools & Commands
  J7 (Register compress/decompress/recompress tools)
  J8 (Add /dcp slash command)
       ↓
Phase D: Persistence & Config
  J9 (Persist DCP state across sessions)
  J10 (Add DCP config to jcode config)
       ↓
Phase E: Testing
  J11 (Integration test: 100-msg session, verify >=30% token reduction)
```

## Verification

```bash
# Build with DCP feature
cargo build --features dcp

# Run existing tests (DCP feature off)
cargo test

# Run with DCP feature
cargo test --features dcp

# Manual test
cargo run --features dcp
# In jcode: /dcp stats
# Use tools, verify compress/decompress works
# Verify compaction still works for token budget management
```

## Files Summary

| File | Action | Task |
|------|--------|------|
| `Cargo.toml` | Modify | J1: dependency |
| `src/lib.rs` | Modify | J2: mod declaration |
| `src/dcp_plugin.rs` | **Create** | J3: DcpPlugin struct |
| `src/agent.rs` | Modify | J4, J5, J6: wire into agent |
| `src/tool/dcp_compress.rs` | **Create** | J7: compress tool |
| `src/tool/mod.rs` | Modify | J7: register tools |
| `src/slash_command.rs` | Modify | J8: /dcp command |
| `src/session.rs` | Modify | J9: persist state |
| `crates/jcode-config-types/src/lib.rs` | Modify | J10: config |
