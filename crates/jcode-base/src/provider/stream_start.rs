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
        let config = crate::config::Config::load().provider;
        let automatic_reset = match provider {
            ActiveProvider::OpenAI => config.openai_auto_banked_reset,
            ActiveProvider::Claude => config.anthropic_auto_fable_reset,
            _ => false,
        };
        if (self.selected_account_for_provider(provider).is_none()
            && active_account_label_for_provider(provider).is_none()
            && !automatic_reset)
            || !self.provider_uses_oauth_accounts(provider)
        {
            return attempt;
        }
        await_stream_start(attempt?).await
    }
}

async fn await_stream_start(mut stream: EventStream) -> Result<EventStream> {
    // A request that never starts (no headers, no acceptance event) must not
    // hang the turn. The idle timeout only applies once bytes are flowing.
    let timeout_secs = crate::config::config()
        .provider
        .stream_start_timeout_secs
        .max(1);
    await_stream_start_with_timeout(stream, std::time::Duration::from_secs(timeout_secs)).await
}

async fn await_stream_start_with_timeout(
    mut stream: EventStream,
    budget: std::time::Duration,
) -> Result<EventStream> {
    let startup_events = tokio::time::timeout(
        budget,
        collect_stream_start(&mut stream),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for provider stream to start"))??;
    // Only transport startup is buffered. Once headers are accepted, keep
    // streaming normally, including long silent reasoning and partial-output
    // rollback behavior. Never replay content already delivered to the agent.
    Ok(Box::pin(
        futures::stream::iter(startup_events).chain(stream),
    ))
}

async fn collect_stream_start(stream: &mut EventStream) -> Result<Vec<Result<StreamEvent>>> {
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
            | StreamEvent::ResolvedModel { .. }
            | StreamEvent::SessionId(_)
            | StreamEvent::UpstreamProvider { .. } => false,
            _ => true,
        };
        startup_events.push(Ok(event));
        if accepted {
            break;
        }
    }
    Ok(startup_events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_start_errors_when_nothing_arrives() {
        // A stream that accepts but emits no events at all must not hang.
        let stream: EventStream = Box::pin(futures::stream::pending());
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            await_stream_start_with_timeout(stream, std::time::Duration::from_millis(20)),
        )
        .await
        .expect("stream_start must not hang when nothing arrives");
        let err = result.err().expect("startup timeout error");
        assert!(err.to_string().contains("Timed out waiting for provider stream to start"));
    }

    #[tokio::test]
    async fn startup_channel_error_reaches_router_after_metadata() {
        let stream: EventStream = Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::ConnectionType {
                connection: "https/sse".into(),
            }),
            Ok(StreamEvent::ResolvedModel {
                model: "upstream-model".into(),
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
