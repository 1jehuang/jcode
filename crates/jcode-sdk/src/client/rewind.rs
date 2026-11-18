use super::*;

impl JcodeClient {
    /// List stable checkpoint IDs available for this attached session.
    pub fn list_rewind_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::RewindCheckpoint>> {
        match self
            .request_ok(ApiRequest::ListRewindCheckpoints {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::RewindCheckpoints { checkpoints, .. } => Ok(checkpoints),
            other => Err(unexpected("rewind_checkpoints", &other)),
        }
    }

    /// Restore a checkpoint. Conversation History is delivered to event
    /// subscribers before this method receives its completion reply.
    pub fn rewind_checkpoint(
        &self,
        session_id: &str,
        checkpoint_id: &str,
        mode: crate::RewindMode,
    ) -> Result<crate::RewindResult> {
        match self
            .request_ok(ApiRequest::RewindCheckpoint {
                session_id: session_id.to_string(),
                checkpoint_id: checkpoint_id.to_string(),
                mode,
            })?
            .event
        {
            ApiEvent::RewindComplete { result, .. } => Ok(result),
            other => Err(unexpected("rewind_complete", &other)),
        }
    }
}
