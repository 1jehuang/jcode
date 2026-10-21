//! Account-control requests sent from the TUI to the server over a `RemoteConnection`.

use super::*;

impl RemoteConnection {
    /// Pin this session (window) to `label`, or unpin it with `None`.
    /// Returns the request id; the server answers it with Done or Error.
    pub async fn set_session_account(
        &mut self,
        provider: &str,
        label: Option<&str>,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SetSessionAccount {
            id,
            provider: provider.to_string(),
            label: label.map(str::to_string),
        })
        .await?;
        Ok(id)
    }

    /// Change the default account for new and unpinned sessions.
    pub async fn set_default_account(&mut self, provider: &str, label: &str) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SetDefaultAccount {
            id,
            provider: provider.to_string(),
            label: label.to_string(),
        })
        .await?;
        Ok(id)
    }

    /// Per-session same-provider account failover (`None` = config default).
    pub async fn set_account_failover(&mut self, enabled: Option<bool>) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SetAccountFailover { id, enabled })
            .await?;
        Ok(id)
    }

    /// Conditionally migrate this session's pin and failover home from a
    /// replaced OAuth login (`label` + `old_identity`) to `new_identity`. The
    /// server changes only a pin/home that exactly equals the old value and
    /// answers `Done` either way; send `notify_auth_changed` afterwards.
    pub async fn replace_session_account_identity(
        &mut self,
        provider: &str,
        label: &str,
        old_identity: Option<&str>,
        new_identity: Option<&str>,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::ReplaceSessionAccountIdentity {
            id,
            provider: provider.to_string(),
            label: label.to_string(),
            old_identity: old_identity.map(str::to_string),
            new_identity: new_identity.map(str::to_string),
        })
        .await?;
        Ok(id)
    }
}
