use super::*;
use crate::message::{ConnectionPhase, StreamEvent};
use futures::StreamExt;

impl MultiProvider {
    pub(super) async fn await_account_stream_start(
        &self,
        provider: ActiveProvider,
        attempt: Result<EventStream>,
    ) -> Result<EventStream> {
        // OAuth HTTP runtimes return their channel before sending the request.
        // An Ok(channel) is not an accepted request: otherwise a subsequent
        // HTTP 429 bypasses the account router and the client retries forever.
        if (self.selected_account_for_provider(provider).is_none()
            && active_account_label_for_provider(provider).is_none())
            || !self.provider_uses_oauth_accounts(provider)
        {
            return attempt;
        }
        await_stream_start(attempt?).await
    }
}

async fn await_stream_start(mut stream: EventStream) -> Result<EventStream> {
    let mut startup_events = Vec::new();
    while let Some(item) = stream.next().await {
        let event = item?;
        if let StreamEvent::Error {
            message,
            retry_after_secs,
        } = event
        {
            return Err(jcode_agent_runtime::StreamError::new(message, retry_after_secs).into());
        }
        let accepted = match &event {
            StreamEvent::ConnectionPhase { phase } => matches!(
                phase,
                ConnectionPhase::WaitingForResponse | ConnectionPhase::Streaming
            ),
            StreamEvent::ConnectionType { .. }
            | StreamEvent::StatusDetail { .. }
            | StreamEvent::ServiceTier { .. }
            | StreamEvent::SessionId(_)
            | StreamEvent::UpstreamProvider { .. } => false,
            _ => true,
        };
        startup_events.push(Ok(event));
        if accepted {
            break;
        }
    }
    // Only transport startup is buffered. Once headers are accepted, keep
    // streaming normally, including long silent reasoning and partial-output
    // rollback behavior. Never replay content already delivered to the agent.
    Ok(Box::pin(
        futures::stream::iter(startup_events).chain(stream),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn startup_channel_error_reaches_router_after_metadata() {
        let stream: EventStream = Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::ConnectionType {
                connection: "https/sse".into(),
            }),
            Ok(StreamEvent::ConnectionPhase {
                phase: ConnectionPhase::SendingRequest,
            }),
            Err(anyhow!("429 Too Many Requests")),
        ]));
        let err = await_stream_start(stream)
            .await
            .err()
            .expect("startup error");
        assert!(err.to_string().contains("429"));
    }

    #[tokio::test]
    async fn startup_error_event_preserves_retry_after() {
        let stream: EventStream = Box::pin(futures::stream::iter(vec![Ok(StreamEvent::Error {
            message: "rate limit exceeded".into(),
            retry_after_secs: Some(60),
        })]));
        let err = await_stream_start(stream)
            .await
            .err()
            .expect("startup error");
        assert_eq!(
            err.downcast_ref::<jcode_agent_runtime::StreamError>()
                .expect("typed stream error")
                .retry_after_secs,
            Some(60)
        );
    }

    #[tokio::test]
    async fn accepted_request_returns_without_waiting_for_output() {
        let stream: EventStream = Box::pin(
            futures::stream::iter(vec![Ok(StreamEvent::ConnectionPhase {
                phase: ConnectionPhase::WaitingForResponse,
            })])
            .chain(futures::stream::pending()),
        );
        let mut accepted = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            await_stream_start(stream),
        )
        .await
        .expect("do not buffer long reasoning")
        .unwrap();
        assert!(matches!(
            accepted.next().await.unwrap().unwrap(),
            StreamEvent::ConnectionPhase {
                phase: ConnectionPhase::WaitingForResponse
            }
        ));
    }

    #[tokio::test]
    async fn accepted_stream_preserves_event_order_and_later_errors() {
        let stream: EventStream = Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::StatusDetail {
                detail: "connecting".into(),
            }),
            Ok(StreamEvent::TextDelta("hello".into())),
            Err(anyhow!("late transport failure")),
        ]));
        let mut accepted = await_stream_start(stream).await.unwrap();
        assert!(matches!(
            accepted.next().await.unwrap().unwrap(),
            StreamEvent::StatusDetail { .. }
        ));
        assert!(
            matches!(accepted.next().await.unwrap().unwrap(), StreamEvent::TextDelta(text) if text == "hello")
        );
        assert!(
            accepted
                .next()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("late transport failure")
        );
    }
}
