//! Elicitation responses for remote `ask_user` forms.
//!
//! A child module can read the private request-id counter and call the private
//! `send_request`, so no visibility change is needed on `RemoteConnection`.
use super::*;

impl RemoteConnection {
    /// Send one answer and return the request id its `Done`/`Error`
    /// acknowledgement will carry.
    pub(crate) async fn send_elicitation_response(
        &mut self,
        session_id: &str,
        elicitation_id: &str,
        response: crate::protocol::elicitation::ElicitationResponse,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::ElicitationResponse {
            id,
            session_id: session_id.to_string(),
            elicitation_id: elicitation_id.to_string(),
            response,
        })
        .await?;
        Ok(id)
    }
}
