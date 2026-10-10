//! Consumer cancellation at transport boundaries, outside credential rotation.
use anyhow::Result;
use jcode_message_types::StreamEvent;
use std::future::Future;
use tokio::sync::mpsc;

pub(super) async fn while_consumer_open<T>(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    operation: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = tx.closed() => None,
        result = operation => Some(result),
    }
}

pub(super) async fn after_authentication<T, F>(
    tx: &mpsc::Sender<Result<StreamEvent>>,
    authenticate: impl Future<Output = Result<String>>,
    transport: impl FnOnce(String) -> F,
) -> Result<Option<T>>
where
    F: Future<Output = T>,
{
    if tx.is_closed() {
        return Ok(None);
    }
    // Rotation may have already happened remotely. Finish persistence and
    // the in-memory commit before observing consumer cancellation again.
    let token = authenticate.await?;
    Ok(while_consumer_open(tx, transport(token)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    #[tokio::test]
    async fn cancellation_authentication_commits_before_transport_is_skipped() {
        let (tx, rx) = mpsc::channel(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let persisted = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(AtomicBool::new(false));
        let called_transport = Arc::new(AtomicBool::new(false));
        let persisted_task = Arc::clone(&persisted);
        let committed_task = Arc::clone(&committed);
        let called_task = Arc::clone(&called_transport);
        let task = tokio::spawn(async move {
            after_authentication(
                &tx,
                async move {
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    persisted_task.store(true, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    committed_task.store(true, Ordering::SeqCst);
                    Ok("synthetic-access".to_string())
                },
                move |_| async move {
                    called_task.store(true, Ordering::SeqCst);
                },
            )
            .await
        });
        started_rx.await.unwrap();
        drop(rx);
        tokio::task::yield_now().await;
        let _ = release_tx.send(());
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            persisted.load(Ordering::SeqCst),
            "consumer cancellation must not discard rotated-token persistence"
        );
        assert!(
            committed.load(Ordering::SeqCst),
            "in-memory commit must finish with persistence"
        );
        assert!(result.is_none());
        assert!(!called_transport.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_authentication_errors_propagate_without_transport() {
        let (tx, _rx) = mpsc::channel(1);
        let result = after_authentication(
            &tx,
            async {
                anyhow::bail!("synthetic refresh failure");
            },
            |_| async { panic!("transport must not start on authentication failure") },
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "synthetic refresh failure");
    }

    #[tokio::test]
    async fn cancellation_closed_consumer_does_not_poll_transport() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let result = while_consumer_open(&tx, async {
            panic!("closed consumer must not poll transport")
        })
        .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cancellation_pending_operation_stops_on_consumer_drop() {
        let (tx, rx) = mpsc::channel(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            while_consumer_open(&tx, async {
                started_tx.send(()).unwrap();
                std::future::pending::<()>().await
            })
            .await
        });
        started_rx.await.unwrap();
        drop(rx);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
}
