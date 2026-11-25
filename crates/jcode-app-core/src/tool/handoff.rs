//! The outer executor owns backgrounding, including nested batch subcalls.
use std::future::Future;

tokio::task_local! {
    static OUTER_OWNER: ();
}

pub(crate) fn scope<F: Future>(call: F) -> impl Future<Output = F::Output> {
    OUTER_OWNER.scope((), call)
}

pub(crate) fn has_outer_owner() -> bool {
    OUTER_OWNER.try_with(|_| ()).is_ok()
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
