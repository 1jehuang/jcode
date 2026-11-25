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
