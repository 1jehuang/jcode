//! Cloud tool capability lifecycle and callback delivery.
use super::*;

impl RemoteConnection {
    pub(crate) fn bind_cloud_authority(&mut self, live: &Arc<std::sync::atomic::AtomicBool>) {
        self.cloud_authority = Some(Arc::downgrade(live));
    }

    pub(super) fn revoke_cloud_authority(&mut self) {
        if let Some(live) = self.cloud_authority.take().and_then(|live| live.upgrade()) {
            live.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        for task in self.cloud_callbacks.drain(..) {
            task.abort();
        }
    }
    pub(crate) fn track_cloud_callback(&mut self, task: &tokio::task::JoinHandle<()>) {
        self.cloud_callbacks.retain(|task| !task.is_finished());
        self.cloud_callbacks.push(task.abort_handle());
    }
    pub(crate) async fn configure_cloud_tools(
        &mut self,
        tools: crate::protocol::SessionToolConfig,
    ) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::ConfigureTools { id, tools })
            .await
    }

    pub(crate) fn cloud_callback_sender(
        &mut self,
    ) -> impl FnOnce(String, String, Option<String>) + Send + 'static {
        let writer = Arc::clone(&self.writer);
        let id = self.next_request_id;
        self.next_request_id += 1;
        move |call_id, output, error| {
            tokio::spawn(async move {
                let request = Request::ToolResult {
                    id,
                    call_id,
                    output,
                    error,
                };
                let result = async {
                    let json = serde_json::to_string(&request)?;
                    let mut writer = writer.lock().await;
                    writer.write_all(format!("{json}\n").as_bytes()).await?;
                    anyhow::Ok(())
                }
                .await;
                if let Err(error) = result {
                    crate::logging::warn(&format!(
                        "Could not deliver cloud tool result: {error:#}"
                    ));
                }
            });
        }
    }

    pub(crate) fn cloud_expiry_revoke(
        live: Arc<std::sync::atomic::AtomicBool>,
        deadline: tokio::time::Instant,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            // ConfigureTools is rejected mid-turn. Revoke local access now,
            // then let the app issue its one-shot cleanup after becoming idle.
            live.store(false, std::sync::atomic::Ordering::SeqCst);
        })
    }
}
