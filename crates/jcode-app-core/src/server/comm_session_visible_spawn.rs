use std::path::PathBuf;

use crate::session::Session;

pub(super) fn create_visible_spawn_session(
    working_dir: Option<&str>,
    model_override: Option<&str>,
    provider_key_override: Option<&str>,
    route_api_method_override: Option<&str>,
    effort_override: Option<&str>,
    selfdev_requested: bool,
) -> anyhow::Result<(String, PathBuf)> {
    // No directory means the spawner has no project, so the worker has none
    // either. Falling back to the daemon's cwd would launch the window inside
    // whichever repository happened to start this process (P2.5).
    let cwd = working_dir
        .filter(|dir| !dir.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot spawn a session with no working directory: the spawner has no \
                 project, so there is no directory to open the session in. Pass an explicit \
                 working_dir, or spawn from a session that has one."
            )
        })?;

    let mut session = Session::create(None, None);
    session.working_dir = Some(cwd.display().to_string());
    if let Some(model) = model_override {
        session.model = Some(model.to_string());
    }
    if let Some(provider_key) = provider_key_override {
        session.provider_key = Some(provider_key.to_string());
    }
    if let Some(route_api_method) = route_api_method_override
        .map(str::trim)
        .filter(|route| !route.is_empty())
    {
        session.route_api_method = Some(route_api_method.to_string());
    }
    if let Some(effort) = effort_override.map(str::trim).filter(|e| !e.is_empty()) {
        // Persisted effort is restored (and validated against the resolved
        // provider/model) by `restore_reasoning_effort_from_session` when the
        // headed client attaches to this session.
        session.reasoning_effort = Some(effort.to_string());
    }
    if selfdev_requested {
        session.set_canary("self-dev");
    }
    // The headed client attaches in a separate process and must find the
    // prepared model/provider/effort on disk, so bypass the untouched-session
    // save gate from 783c979a0.
    session.save_prepared()?;

    Ok((session.id.clone(), cwd))
}
