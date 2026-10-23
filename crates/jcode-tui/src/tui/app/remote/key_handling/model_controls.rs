use super::*;

pub(super) async fn apply_remote_speed_direction(
    app: &mut App,
    remote: &mut RemoteConnection,
    direction: i8,
) -> Result<()> {
    use jcode_provider_core::service_tier::{speed_tier_ladder_with_capabilities, step_speed_tier};
    let (provider_name, provider_model) = app.remote_effort_identity();
    let ladder = speed_tier_ladder_with_capabilities(
        provider_name.as_deref(),
        provider_model.as_deref(),
        app.remote_available_service_tiers.as_deref(),
    );
    let Some((index, next, at_end)) =
        step_speed_tier(&ladder, app.remote_service_tier.as_deref(), direction)
    else {
        app.set_status_notice("Speed tiers not available for this model");
        return Ok(());
    };
    if at_end {
        app.set_status_notice(app_mod::speed_tier_notice(
            next,
            index,
            ladder.len(),
            Some(direction),
        ));
        return Ok(());
    }
    request_remote_service_tier(app, remote, next).await?;
    Ok(())
}

pub(super) async fn request_remote_service_tier(
    app: &mut App,
    remote: &mut RemoteConnection,
    tier: &str,
) -> Result<()> {
    // A transient notice acknowledges input without claiming configuration or
    // provider acceptance. The server acknowledgement replaces it, so fast
    // round trips do not add a second transcript message.
    app.set_status_notice(format!(
        "Speed: {} requested (waiting for server)",
        app_mod::service_tier_display_label(tier)
    ));
    remote.set_service_tier(tier).await
}

pub(super) async fn apply_remote_effort_direction(
    app: &mut App,
    remote: &mut RemoteConnection,
    direction: i8,
) -> Result<()> {
    // Use the best-known provider/model identity (server-reported when
    // available, header hints during the pre-History bootstrap window) so
    // effort cycling works immediately after spawn instead of claiming the
    // provider does not support it until the History payload settles.
    let (provider_name, provider_model) = app.remote_effort_identity();
    let efforts =
        app_mod::inferred_reasoning_efforts(provider_name.as_deref(), provider_model.as_deref());
    if efforts.is_empty() {
        app.set_status_notice("Reasoning effort not available for this provider");
        return Ok(());
    }
    let current = app.remote_reasoning_effort_hint();
    let current = current.as_deref();
    let current_index = current
        .and_then(|c| efforts.iter().position(|e| *e == c))
        .unwrap_or(efforts.len() - 1);
    let len = efforts.len();
    let next_index = if direction > 0 {
        if current_index + 1 >= len {
            current_index
        } else {
            current_index + 1
        }
    } else if current_index == 0 {
        0
    } else {
        current_index - 1
    };
    let next_effort = efforts[next_index];
    if Some(next_effort) == current {
        let label = app_mod::effort_display_label(next_effort);
        app.set_status_notice(format!(
            "Effort: {} (already at {})",
            label,
            if direction > 0 { "max" } else { "min" }
        ));
    } else {
        app.remote_reasoning_effort = Some(next_effort.to_string());
        app.invalidate_model_picker_cache();
        app.set_status_notice(format!(
            "Effort: {} (will apply to next request)",
            app_mod::effort_display_label(next_effort)
        ));
        remote.set_reasoning_effort(next_effort).await?;
    }
    Ok(())
}
