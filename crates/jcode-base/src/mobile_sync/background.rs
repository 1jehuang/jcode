use super::{PERSISTED, SYNC_DEBOUNCE, SYNC_INTERVAL, SyncTiming, daemon_http_client, sync_pass};

pub(super) async fn background_sync() {
    let client = daemon_http_client();
    let mut ticks = tokio::time::interval(SYNC_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let reconcile = tokio::select! {
            _ = ticks.tick() => true,
            _ = PERSISTED.notified() => {
                // A bounded coalescing window, not a trailing debounce:
                // continuous token saves cannot postpone sync forever.
                let mut debounce = SyncTiming::new("debounce_wait");
                tokio::time::sleep(SYNC_DEBOUNCE).await;
                debounce.outcome = "success";
                false
            }
        };
        // sync_pass records failures and disabled skips without error text.
        if sync_pass(&client, reconcile).await.is_err() {
            crate::logging::debug("mobile sync pass failed (see stage timing)");
        }
    }
}
