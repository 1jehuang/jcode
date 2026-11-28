//! Server-side persistence of confirmed model switches to config.toml.

/// Persist a server-confirmed model switch as the config default so new
/// sessions inherit the user's last explicit choice.
///
/// Runs on the server because the server owns the `config.toml` that its new
/// sessions read; over SSH the client's config is a different file and must
/// not be the write target. Called only after the switch was confirmed for
/// the request that asked for persistence, so a rejected `/model B` reply can
/// never overwrite the default: the reply's own request decides, not
/// whichever reply happens to land first.
///
/// Honors `[provider].persist_model_switch = false` and skips the write when
/// the file already carries this exact default (file-only compare, so a
/// `JCODE_MODEL` env override cannot mask a real change).
pub(super) fn persist_confirmed_model_default(model_spec: &str, provider_key: Option<&str>) {
    let cfg = crate::config::Config::load();
    if !cfg.provider.persist_model_switch {
        crate::logging::info(&format!(
            "Skipped persisting model switch because provider.persist_model_switch=false: {}",
            model_spec
        ));
        return;
    }
    match crate::config::Config::set_default_model_if_changed(Some(model_spec), provider_key) {
        Ok(true) => {
            crate::logging::info(&format!(
                "Persisted confirmed model switch to config.toml: {} via {} (survives relaunch)",
                model_spec,
                provider_key.unwrap_or("auto")
            ));
        }
        Ok(false) => {}
        Err(e) => {
            crate::logging::warn(&format!(
                "Failed to persist model switch to config.toml: {}",
                e
            ));
        }
    }
}
