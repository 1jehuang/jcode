//! On Peer Disconnect Hook Handler
//! Category: Collaboration Events

use anyhow::Result;
use tracing;

/// On Peer Disconnect hook implementation
pub struct OnPeerDisconnectHook;

impl OnPeerDisconnectHook {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookHandler for OnPeerDisconnectHook {
    async fn handle(&self, event: &crate::hooks::HookEvent) -> Result<()> {
        tracing::info!("Handling event in on_peer_disconnect hook");

        // TODO: Implement on_peer_disconnect hook logic
        match event {
            // Handle specific event types
            _ => {
                tracing::debug!("on_peer_disconnect received generic event");
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "on_peer_disconnect"
    }

    fn priority(&self) -> u32 {
        100  // Default priority, adjust as needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_on_peer_disconnect_basic() {
        let hook = OnPeerDisconnectHook::new();
        assert_eq!(hook.name(), "on_peer_disconnect");
        assert_eq!(hook.priority(), 100);
    }
}
