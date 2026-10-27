//! Targeted plugin-source reconciliation. Ordinary and dynamic MCP entries stay
//! untouched, including their live connections and cached tool proxies.

use super::*;
use crate::mcp::is_plugin_mcp_source;
use std::collections::BTreeSet;

impl McpManager {
    /// Reconcile enabled plugin MCP sources without dropping unrelated dynamic
    /// servers or restarting non-plugin connections. Called after plugin changes
    /// and at prompt boundaries when an external registry edit is detected.
    pub async fn reload_plugin_sources(&mut self) -> Result<(usize, Vec<(String, String)>)> {
        let fresh = McpConfig::load_for_dir(self.project_dir.as_deref());
        let old_names: BTreeSet<String> = self
            .config
            .sources
            .iter()
            .filter(|(_, source)| is_plugin_mcp_source(source))
            .map(|(name, _)| name.clone())
            .collect();
        let mut incoming = BTreeMap::new();
        for (name, config) in &fresh.servers {
            if !fresh
                .sources
                .get(name)
                .is_some_and(|source| is_plugin_mcp_source(source))
            {
                continue;
            }
            // An explicitly configured or dynamically connected server wins a
            // collision, even when its name resembles a plugin namespace.
            if self.config.servers.contains_key(name) && !old_names.contains(name) {
                continue;
            }
            incoming.insert(name.clone(), config.clone());
        }
        let mut changed = BTreeSet::new();
        for name in old_names.iter().chain(incoming.keys()) {
            let same = match (self.config.servers.get(name), incoming.get(name)) {
                (Some(old), Some(new)) => {
                    serde_json::to_value(old)? == serde_json::to_value(new)?
                        && self.config.sources.get(name) == fresh.sources.get(name)
                        && self.config.pending_approval.contains(name)
                            == fresh.pending_approval.contains(name)
                }
                _ => false,
            };
            if !same {
                changed.insert(name.clone());
            }
        }
        for name in &changed {
            self.disconnect(name).await?;
            self.config.servers.remove(name);
            self.config.sources.remove(name);
            self.config.pending_approval.remove(name);
            self.last_errors
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(name);
        }
        for name in &changed {
            if let Some(config) = incoming.get(name) {
                self.config.servers.insert(name.clone(), config.clone());
                if let Some(source) = fresh.sources.get(name) {
                    self.config.sources.insert(name.clone(), source.clone());
                }
                if fresh.pending_approval.contains(name) {
                    self.config.pending_approval.insert(name.clone());
                }
            }
        }
        let mut successes = 0;
        let mut failures = Vec::new();
        for name in changed {
            if let Some(config) = self
                .config
                .servers
                .get(&name)
                .filter(|config| config.is_enabled())
            {
                match self.connect(&name, config).await {
                    Ok(()) => successes += 1,
                    Err(error) => failures.push((name, format!("{error:#}"))),
                }
            }
        }
        Ok((successes, failures))
    }
}
