//! DCP Plugin — wraps ContextPruner and bridges jcode ↔ DCP types.

use crate::message::Message as JMsg;
#[cfg(feature = "dcp")]
use dynamic_context_pruning::{Config, ContextPruner};

/// DCP plugin that wraps ContextPruner and handles type conversion.
pub struct DcpPlugin {
    pruner: ContextPruner,
    enabled: bool,
}

impl DcpPlugin {
    /// Create a new DCP plugin with default config.
    pub fn new() -> Result<Self, String> {
        let config = Config::default();
        let pruner = ContextPruner::new(config).map_err(|e| format!("DCP init failed: {e:?}"))?;
        Ok(Self {
            pruner,
            enabled: true,
        })
    }

    /// Create with custom config.
    pub fn with_config(config: Config) -> Result<Self, String> {
        let pruner = ContextPruner::new(config).map_err(|e| format!("DCP init failed: {e:?}"))?;
        Ok(Self {
            pruner,
            enabled: true,
        })
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
    pub fn transform(&mut self, messages: &[JMsg]) -> Result<DcpTransformOutput, String> {
        if !self.enabled || messages.is_empty() {
            return Ok(DcpTransformOutput {
                messages: messages.to_vec(),
                tokens_saved: 0,
                removed_count: 0,
                changed: false,
            });
        }

        // 1. Convert jcode → DCP
        let dcp_messages = crate::dcp_bridge::jcode_to_dcp(messages);

        // 2. Run DCP transform with diff
        let result = self
            .pruner
            .transform_messages_with_diff(dcp_messages)
            .map_err(|e| format!("DCP transform error: {e:?}"))?;

        // 3. Convert DCP → jcode
        let jcode_messages = crate::dcp_bridge::dcp_to_jcode(result.messages);

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
