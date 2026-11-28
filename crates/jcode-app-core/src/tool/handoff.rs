//! The outer executor owns backgrounding, including nested batch subcalls.
use std::future::Future;

tokio::task_local! {
    static OUTER_OWNER: tokio::sync::watch::Receiver<Option<String>>;
}

pub(crate) fn current_owner() -> Option<tokio::sync::watch::Receiver<Option<String>>> {
    OUTER_OWNER
        .try_with(|owner| Some(owner.clone()))
        .unwrap_or(None)
}

pub(crate) fn scope_with_owner<F: Future>(
    owner: tokio::sync::watch::Receiver<Option<String>>,
    call: F,
) -> impl Future<Output = F::Output> {
    OUTER_OWNER.scope(owner, call)
}

pub(crate) fn scope_with_task<F: Future>(
    call: F,
) -> (
    tokio::sync::watch::Sender<Option<String>>,
    impl Future<Output = F::Output>,
) {
    let (task, owner) = tokio::sync::watch::channel(None);
    (task, scope_with_owner(owner, call))
}

pub(crate) fn scope<F: Future>(call: F) -> impl Future<Output = F::Output> {
    let owner = match current_owner() {
        Some(owner) => owner,
        None => tokio::sync::watch::channel(None).1,
    };
    scope_with_owner(owner, call)
}

pub(crate) fn has_outer_owner() -> bool {
    OUTER_OWNER.try_with(|_| ()).is_ok()
}

/// Attach to the existing task only after its outer executor promotes it.
pub(crate) fn on_backgrounded<F, Fut>(attach: F)
where
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(mut owner) = current_owner() else {
        return;
    };
    tokio::spawn(async move {
        loop {
            let task_id = owner.borrow_and_update().clone();
            if let Some(task_id) = task_id {
                attach(task_id).await;
                return;
            }
            if owner.changed().await.is_err() {
                return;
            }
        }
    });
}

pub(crate) const BACKGROUND_WAIT_GUIDANCE: &str = "Meanwhile, work on anything that does not depend on its result. Do not start a second copy or change inputs it is still using.";

/// What a backgrounded task's reply tells the model about its completion.
pub(crate) fn background_delivery_notice(wake: bool) -> &'static str {
    if wake {
        "You will be notified when it completes. Do not poll or sleep while waiting."
    } else {
        "You will not be notified when it completes; check on it with `bg` when you need its result."
    }
}
