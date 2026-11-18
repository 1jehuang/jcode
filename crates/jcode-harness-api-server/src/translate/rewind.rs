use super::*;

impl BridgeState {
    pub(super) fn rewind_request(
        &mut self,
        request: &Value,
        req: &str,
        api_id: u64,
    ) -> Vec<Outbound> {
        let id = self.legacy_id();
        let kind = if req == "list_rewind_checkpoints" {
            SimpleKind::RewindCheckpoints
        } else {
            SimpleKind::RewindComplete
        };
        self.pending_simple.push((id, api_id, kind));
        let mut legacy = json!({"type": req, "id": id});
        if req == "rewind_checkpoint" {
            legacy["checkpoint_id"] = request["checkpoint_id"].clone();
            legacy["mode"] = request["mode"].clone();
        }
        vec![Outbound::Legacy(legacy)]
    }

    pub(super) fn rewind_event(&mut self, event: &Value) -> Vec<ServerFrame> {
        let kind = if event["type"] == "rewind_checkpoints" {
            SimpleKind::RewindCheckpoints
        } else {
            SimpleKind::RewindComplete
        };
        let id = event["id"].as_u64().unwrap_or(0);
        let Some(api_id) = self.take_simple(id, kind) else {
            return vec![];
        };
        let mut wire = event.clone();
        wire["ev"] = event["type"].clone();
        let translated =
            serde_json::from_value::<ApiEvent>(wire).unwrap_or_else(|error| ApiEvent::Error {
                code: ErrorCode::InvalidRequest,
                message: format!("Invalid rewind response: {error}"),
            });
        vec![ServerFrame::reply(api_id, translated)]
    }

    /// History precedes completion and must not consume the pending SDK reply.
    pub(super) fn rewind_history_pending(&self, id: u64) -> bool {
        self.pending_simple
            .iter()
            .any(|(pending_id, _, kind)| *pending_id == id && *kind == SimpleKind::RewindComplete)
    }
}
