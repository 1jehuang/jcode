use super::*;

impl RemoteConnection {
    pub async fn set_route_selection(
        &mut self,
        selection: crate::provider::RouteSelection,
    ) -> Result<u64> {
        self.set_route_selection_verified(selection, None).await
    }

    pub async fn set_route_selection_verified(
        &mut self,
        selection: crate::provider::RouteSelection,
        verification: Option<jcode_provider_core::AuthRouteVerification>,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SetRoute {
            id,
            selection,
            automatic_failover: false,
            verification,
        })
        .await?;
        Ok(id)
    }

    /// Apply an automatic target only if the daemon still approves its pool membership.
    pub async fn set_route_selection_for_failover(
        &mut self,
        selection: crate::provider::RouteSelection,
    ) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SetRoute {
            id,
            selection,
            automatic_failover: true,
            verification: None,
        })
        .await?;
        Ok(id)
    }
}
