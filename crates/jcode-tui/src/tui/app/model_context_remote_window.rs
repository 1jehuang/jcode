use super::*;

impl App {
    /// Context window for a remote session.
    ///
    /// A window the server reported for the same model wins over the static
    /// catalog, which can describe a different deployment of a known model. A
    /// different model drops the saved window. Without one, the catalog resolves
    /// the models it knows. A model chosen server-side is not in the catalog, and
    /// the client's own provider is an inert placeholder whose context_window()
    /// is the generic 200_000 default, so keep the last reported value instead
    /// of overwriting it with the default.
    pub(super) fn remote_context_limit(
        &mut self,
        model: &str,
        server_window: Option<u64>,
    ) -> usize {
        if let Some(window) = server_window {
            self.remote_server_context = Some((model.to_string(), window as usize));
            return window as usize;
        }
        match &self.remote_server_context {
            Some((saved_model, limit)) if saved_model == model => *limit,
            _ => {
                self.remote_server_context = None;
                crate::provider::context_limit_for_model_with_provider(
                    model,
                    self.remote_provider_name.as_deref(),
                )
                .unwrap_or(self.context_limit as usize)
            }
        }
    }

    /// Apply the window from a server model update. An older server may omit
    /// the window on a route switch, so a missing value drops the saved one.
    pub(in crate::tui::app) fn apply_server_context_window(
        &mut self,
        model: &str,
        server_window: Option<u64>,
    ) {
        self.remote_server_context = None;
        self.update_context_limit_for_model(model, server_window);
    }
}
