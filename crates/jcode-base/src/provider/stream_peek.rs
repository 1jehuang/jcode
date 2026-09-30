//! Peek at the start of a completion stream before anything reaches the
//! consumer.
//!
//! Runtimes return `Ok(stream)` at once and report HTTP errors *inside* the
//! stream, so provider-level failover never sees them as `Err`. Reading up to
//! the HTTP response (connection bookkeeping events only) tells whether the
//! request failed before any model output. That is the only point where a
//! resend (another profile, another account) cannot duplicate output.
//!
//! Used by credit failover (402 between OpenAI-compatible profiles) and by
//! same-provider account failover (usage-limit 429 between stored accounts).

use crate::message::{ConnectionPhase, StreamEvent};
use anyhow::Result;
use futures::StreamExt;
use jcode_provider_core::EventStream;

pub(super) enum Peeked {
    /// The response started (or the stream ended) without a pre-response error.
    Stream(EventStream),
    /// The request failed before any model output.
    Failed {
        error: anyhow::Error,
        /// Billing exhaustion (HTTP 402 or credit wording without a reset time).
        out_of_credit: bool,
        /// Subscription usage spent on this account (stable marker, see
        /// `jcode_provider_core::usage_limit`).
        usage_limit: Option<jcode_provider_core::AccountUsageLimit>,
        /// The exact stream as the runtime produced it, for callers that do
        /// not fail over and must surface the error unchanged.
        replay: EventStream,
    },
}

/// Read the stream up to the HTTP response. Errors come either as `Err` items
/// (non-2xx HTTP status) or as `StreamEvent::Error` (an HTTP 200 SSE stream
/// whose first event is an error payload).
pub(super) async fn peek_before_output(mut stream: EventStream) -> Peeked {
    let mut buffered: Vec<Result<StreamEvent>> = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(StreamEvent::Error {
                message,
                retry_after_secs,
            }) => {
                let out_of_credit =
                    jcode_provider_core::is_billing_exhausted_error_message(&message);
                let usage_limit = jcode_provider_core::classify_account_usage_limit(&message);
                let error = anyhow::anyhow!(message.clone());
                buffered.push(Ok(StreamEvent::Error {
                    message,
                    retry_after_secs,
                }));
                return Peeked::Failed {
                    error,
                    out_of_credit,
                    usage_limit,
                    replay: Box::pin(futures::stream::iter(buffered).chain(stream)),
                };
            }
            Ok(event) => {
                let still_connecting = matches!(
                    event,
                    StreamEvent::ConnectionType { .. }
                        | StreamEvent::StatusDetail { .. }
                        | StreamEvent::UpstreamProvider { .. }
                        | StreamEvent::ConnectionPhase {
                            phase: ConnectionPhase::Authenticating
                                | ConnectionPhase::Connecting
                                | ConnectionPhase::SendingRequest
                                | ConnectionPhase::WaitingForResponse
                                | ConnectionPhase::Retrying { .. }
                        }
                );
                buffered.push(Ok(event));
                if !still_connecting {
                    break;
                }
            }
            Err(error) => {
                let text = format!("{error:#}");
                let out_of_credit = jcode_provider_core::is_billing_exhausted_error_message(&text);
                let usage_limit = jcode_provider_core::classify_account_usage_limit(&text);
                buffered.push(Err(error));
                return Peeked::Failed {
                    error: anyhow::anyhow!(text),
                    out_of_credit,
                    usage_limit,
                    replay: Box::pin(futures::stream::iter(buffered).chain(stream)),
                };
            }
        }
    }
    Peeked::Stream(Box::pin(futures::stream::iter(buffered).chain(stream)))
}
