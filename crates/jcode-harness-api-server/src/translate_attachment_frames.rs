use super::*;

impl BridgeState {
    /// Tool identity of a daemon `tool` history row, if it carries one.
    pub(super) fn history_tool(message: &Value) -> Option<jcode_harness_api::HistoryTool> {
        let data = &message["tool_data"];
        let id = data["id"].as_str().filter(|id| !id.is_empty())?;
        (message["role"] == "tool").then(|| jcode_harness_api::HistoryTool {
            id: id.to_string(),
            name: data["name"].as_str().unwrap_or("").to_string(),
            input: data["input"].clone(),
        })
    }

    pub(super) fn attachment_recovery(event: &Value) -> Option<ServerFrame> {
        if event["messages"].as_array().is_none_or(Vec::is_empty)
            || event["activity"]["is_processing"].as_bool() == Some(true)
        {
            return None;
        }
        let directive = &event["reload_recovery"];
        let (continuation_message, reconnect_notice) = if let Some(message) =
            directive["continuation_message"].as_str()
        {
            (
                message.to_string(),
                directive["reconnect_notice"].as_str().map(str::to_string),
            )
        } else if event["was_interrupted"].as_bool() == Some(true) {
            // Compatibility with daemons predating reload_recovery. Keep in
            // sync with ReloadContext::interrupted_session_continuation_message.
            ("Your session was interrupted by a server reload while a tool was running. The tool was aborted and results may be incomplete. Continue exactly where you left off and do not ask the user what to do next.".to_string(), None)
        } else {
            return None;
        };
        if continuation_message.trim().is_empty() {
            return None;
        }
        Some(ServerFrame::event(ApiEvent::SessionRecovery {
            session_id: event["session_id"].as_str()?.to_string(),
            continuation_message,
            reconnect_notice,
        }))
    }

    pub(super) fn applet_frame(session_id: &str, snapshot: &Value) -> Option<ServerFrame> {
        if session_id.is_empty() {
            return None;
        }
        // Missing means empty; malformed must not clear a valid snapshot.
        let snapshot = if snapshot.is_null() {
            jcode_harness_api::jcode_applet_types::AgentApplets::default()
        } else {
            serde_json::from_value(snapshot.clone()).ok()?
        };
        Some(ServerFrame::event(ApiEvent::AppletState {
            session_id: session_id.to_string(),
            snapshot,
        }))
    }

    pub(super) fn side_panel_frame(session_id: &str, snapshot: &Value) -> Option<ServerFrame> {
        if session_id.is_empty() {
            return None;
        }
        // Missing history state means empty: native history omits empty panels.
        // Malformed snapshots must not clear a previously valid document.
        let snapshot = if snapshot.is_null() {
            jcode_harness_api::SidePanelSnapshot::default()
        } else {
            serde_json::from_value(snapshot.clone()).ok()?
        };
        Some(ServerFrame::event(ApiEvent::SidePanelState {
            session_id: session_id.to_string(),
            snapshot,
        }))
    }
}
