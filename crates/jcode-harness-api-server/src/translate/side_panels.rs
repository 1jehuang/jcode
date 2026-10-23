//! Keep panel attachment identity consistent across both protocol envelopes.
use super::*;

impl BridgeState {
    pub(super) fn side_panel_frame(session_id: &str, snapshot: &Value) -> Option<ServerFrame> {
        if session_id.is_empty() {
            return None;
        }
        // Missing history state means empty: native history omits empty panels.
        // Malformed snapshots must not clear a previously valid document.
        let snapshot = if snapshot.is_null() {
            jcode_harness_api::SidePanelSnapshot::default()
        } else {
            let Ok(snapshot) = serde_json::from_value(snapshot.clone()) else {
                // Malformed input has no authority to replace a valid document.
                return None;
            };
            snapshot
        };
        if snapshot
            .session_id
            .as_deref()
            .is_some_and(|declared| declared != session_id)
        {
            return None;
        }
        Some(ServerFrame::event(ApiEvent::SidePanelState {
            session_id: session_id.to_string(),
            snapshot,
        }))
    }
}
