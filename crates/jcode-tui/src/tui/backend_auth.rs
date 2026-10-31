//! Remote account and authentication state notifications.
use super::*;

impl RemoteConnection {
    pub async fn notify_auth_changed(&mut self) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::NotifyAuthChanged {
            id,
            provider: None,
            auth: None,
            prefer_strongest: false,
        })
        .await
    }

    /// Notify the server about a typed auth lifecycle change and report
    /// transport failure to the caller.
    pub async fn notify_auth_changed_event(
        &mut self,
        provider: Option<&str>,
        auth: Option<AuthChanged>,
        prefer_strongest: bool,
    ) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::NotifyAuthChanged {
            id,
            provider: provider.map(str::to_string),
            auth,
            prefer_strongest,
        })
        .await
    }

    /// Notify the server about auth changes without blocking the caller.
    pub fn notify_auth_changed_detached(&mut self) {
        self.notify_auth_changed_for_provider_detached(None);
    }

    /// Notify the server about a provider-specific auth change without blocking the caller.
    pub fn notify_auth_changed_for_provider_detached(&mut self, provider: Option<&str>) {
        self.notify_auth_changed_detached_event(provider, None, false);
    }

    /// Notify the server about a typed auth lifecycle change without blocking the caller.
    pub fn notify_auth_changed_detached_event(
        &mut self,
        provider: Option<&str>,
        auth: Option<AuthChanged>,
        prefer_strongest: bool,
    ) {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request_detached(
            Request::NotifyAuthChanged {
                id,
                provider: provider.map(str::to_string),
                auth,
                prefer_strongest,
            },
            "notify_auth_changed",
        );
    }

    /// Ask server to switch active Anthropic account for this process/session.
    pub async fn switch_anthropic_account(&mut self, label: &str) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SwitchAnthropicAccount {
            id,
            label: label.to_string(),
        })
        .await
    }

    /// Ask server to switch active OpenAI account for this process/session.
    pub async fn switch_openai_account(&mut self, label: &str) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SwitchOpenAiAccount {
            id,
            label: label.to_string(),
        })
        .await
    }
}
