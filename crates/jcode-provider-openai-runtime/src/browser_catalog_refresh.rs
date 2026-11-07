use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};

static OPTIONAL_BROWSER_REFRESH_RUNNING: AtomicBool = AtomicBool::new(false);

struct OptionalBrowserRefreshGuard;
impl Drop for OptionalBrowserRefreshGuard {
    fn drop(&mut self) {
        OPTIONAL_BROWSER_REFRESH_RUNNING.store(false, Ordering::Release);
    }
}

pub(crate) fn spawn_optional_browser_catalog_refresh<F>(refresh: F)
where
    F: std::future::Future<
            Output = Result<Vec<jcode_base::provider::chatgpt_web::ChatGptWebModel>>,
        > + Send
        + 'static,
{
    if OPTIONAL_BROWSER_REFRESH_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let guard = OptionalBrowserRefreshGuard;
        tokio::spawn(async move {
            let _guard = guard;
            match refresh.await {
                Ok(_) => jcode_base::bus::Bus::global().publish_models_updated(),
                Err(error) => jcode_base::logging::info(&format!(
                    "ChatGPT web models are unavailable: {error:#}"
                )),
            }
        });
    }
}
