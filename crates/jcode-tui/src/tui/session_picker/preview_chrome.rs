//! Selected-session chrome. Never consult the App behind the picker: its
//! credentials, git tree, quotas and in-flight work belong to another session.
use super::*;
use crate::tui::info_widget::{self, InfoWidgetData};

/// Only durable facts are eligible for preview widgets. In particular the
/// character-based transcript estimate is not an API context measurement.
pub(super) fn persisted_info(session: &Session) -> InfoWidgetData {
    persisted_info_from_snapshot(session, crate::session_telemetry::load_context(session))
}

/// Explicit snapshot input prevents cloud previews from consulting local state.
pub(super) fn persisted_info_from_snapshot(
    session: &Session,
    snapshot: Option<crate::session_telemetry::SessionTelemetry>,
) -> InfoWidgetData {
    let snapshot = snapshot.filter(|snapshot| snapshot.matches_session(session));
    // Stored per-message prompt counts do not carry model/account attribution.
    // A session may have switched routes since that message. Only a snapshot
    // with exact route and compaction identity is eligible for context chrome.
    let context_tokens = snapshot.as_ref().map(|s| s.context_tokens);
    let (auth_method, usage_info) = snapshot
        .as_ref()
        .map(snapshot_account_info)
        .unwrap_or_default();
    InfoWidgetData {
        auth_method,
        usage_info,
        model: session.model.clone(),
        provider_name: session.provider_key.clone(),
        working_dir: session.working_dir.clone(),
        session_name: session.short_name.clone(),
        reasoning_effort: session.reasoning_effort.clone(),
        original_working_dir: session.original_working_dir.clone(),
        context_limit: session
            .context_cap
            .or_else(|| snapshot.as_ref().and_then(|s| s.context_limit)),
        observed_context_tokens: context_tokens,
        historical_context: context_tokens.is_some(),
        ..Default::default()
    }
}

/// Account telemetry is usable only for the credential and label actually bound
/// by the daemon. Historical quota expires, even if the transcript is timeless.
fn snapshot_account_info(
    snapshot: &crate::session_telemetry::SessionTelemetry,
) -> (info_widget::AuthMethod, Option<info_widget::UsageInfo>) {
    use info_widget::{AuthMethod, UsageInfo, UsageProvider};
    use jcode_provider_core::ResolvedCredential;
    let provider = snapshot
        .serving_provider
        .as_deref()
        .map(str::to_ascii_lowercase);
    let auth = match (provider.as_deref(), snapshot.credential) {
        (Some("openai"), Some(ResolvedCredential::Oauth)) => AuthMethod::OpenAIOAuth,
        (Some("claude" | "anthropic"), Some(ResolvedCredential::Oauth)) => {
            AuthMethod::AnthropicOAuth
        }
        (_, Some(ResolvedCredential::ApiKey)) => AuthMethod::ApiKey,
        _ => AuthMethod::Unknown,
    };
    let usage_provider = match auth {
        AuthMethod::OpenAIOAuth => UsageProvider::OpenAI,
        AuthMethod::AnthropicOAuth => UsageProvider::Anthropic,
        _ => return (auth, None),
    };
    let age = chrono::Utc::now()
        .signed_duration_since(snapshot.measured_at)
        .num_seconds();
    let usage = snapshot
        .account_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .zip(snapshot.account_usage.as_ref())
        .filter(|(label, usage)| *label == usage.label && usage.error.is_none())
        .filter(|_| (0..=900).contains(&age))
        .filter(|(_, usage)| usage.five_hour_ratio.is_some() || usage.seven_day_ratio.is_some())
        .filter(|(_, usage)| {
            [usage.five_hour_ratio, usage.seven_day_ratio]
                .into_iter()
                .flatten()
                .all(|ratio| ratio.is_finite() && (0.0..=1.0).contains(&ratio))
        })
        .map(|(_, usage)| UsageInfo {
            provider: usage_provider,
            primary_limit_label: usage.five_hour_ratio.and(usage.primary_label.clone()),
            five_hour: usage.five_hour_ratio.unwrap_or(0.0),
            five_hour_resets_at: usage.five_hour_ratio.and(usage.resets_at.clone()),
            secondary_limit_label: usage.seven_day_ratio.and(usage.secondary_label.clone()),
            seven_day: usage.seven_day_ratio.unwrap_or(0.0),
            seven_day_resets_at: usage
                .seven_day_ratio
                .filter(|_| usage.five_hour_ratio.is_none())
                .and(usage.resets_at.clone()),
            available: true,
            ..Default::default()
        });
    (auth, usage)
}

fn selected_info(session: &SessionInfo, extras: Option<&PreviewExtras>) -> InfoWidgetData {
    let mut data = extras
        .and_then(|extras| extras.info_data.clone())
        .unwrap_or_else(|| InfoWidgetData {
            session_name: Some(session.short_name.clone()),
            ..Default::default()
        });
    // Index identity may be fresher. Never carry observations across routes.
    if (session.model.is_some() && session.model != data.model)
        || (session.provider_key.is_some() && session.provider_key != data.provider_name)
    {
        data.observed_context_tokens = None;
        data.context_info = None;
        data.context_limit = None;
        data.historical_context = false;
        data.usage_info = None;
        data.auth_method = info_widget::AuthMethod::Unknown;
    }
    data.model = session.model.clone().or(data.model);
    data.provider_name = session.provider_key.clone().or(data.provider_name);
    data.working_dir = session.working_dir.clone().or(data.working_dir);
    data.session_name = Some(session.short_name.clone());
    data
}

fn next_prompt_number(session: &SessionInfo, extras: Option<&PreviewExtras>) -> usize {
    let visible_users = session
        .messages_preview
        .iter()
        .filter(|message| message.role == "user" && !message.content.trim().is_empty())
        .count();
    let preview_users = extras
        .map_or(0, |extras| extras.hidden_prompts)
        .saturating_add(visible_users);
    // The index count already includes prompts outside the preview window.
    // Use the window as a lower bound when an index snapshot is stale, not as
    // an additional offset that would count hidden prompts twice.
    session
        .user_message_count
        .max(preview_users)
        .saturating_add(1)
}

/// Draw only the image rows intersecting the scrolling transcript. Raster
/// images retain their original fit box when partially scrolled off screen.
pub(super) fn draw_images(
    frame: &mut Frame,
    area: Rect,
    regions: &[jcode_tui_messages::ImageRegion],
    source_rows: &[usize],
    reserved: Option<Rect>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for region in regions {
        let start = source_rows.partition_point(|row| *row < region.abs_line_idx);
        let end = source_rows.partition_point(|row| *row < region.end_line);
        if region.height == 0 || start >= end {
            continue;
        }
        let source_start = source_rows[start];
        let mut image_area = Rect {
            x: area.x,
            y: area.y + start as u16,
            width: area.width,
            height: (end - start).min(u16::MAX as usize) as u16,
        };
        if let Some(panel) = reserved
            && image_area.y < panel.bottom()
            && panel.y < image_area.bottom()
        {
            image_area.width = panel
                .x
                .saturating_sub(area.x)
                .saturating_sub(1)
                .min(area.width);
        }
        if image_area.width == 0 {
            continue;
        }
        let fit = region.render == jcode_tui_messages::ImageRegionRender::Fit;
        if fit {
            let cols = if region.width == 0 {
                image_area.width
            } else {
                region.width.min(image_area.width)
            };
            if !super::super::ui::inline_image_ui::ensure_drawable(region.hash, cols, region.height)
            {
                continue;
            }
            if !crate::tui::mermaid::render_image_widget_fit_stable(
                region.hash,
                image_area,
                frame.buffer_mut(),
                cols,
                region.height,
                (source_start - region.abs_line_idx).min(u16::MAX as usize) as u16,
                false,
                true,
            ) {
                crate::tui::mermaid::render_image_widget_fit(
                    region.hash,
                    image_area,
                    frame.buffer_mut(),
                    false,
                    true,
                );
            }
        } else {
            let rows = crate::tui::mermaid::render_image_widget(
                region.hash,
                image_area,
                frame.buffer_mut(),
                false,
                source_start > region.abs_line_idx,
            );
            if rows == 0 && source_start == region.abs_line_idx {
                frame.render_widget(Paragraph::new("↗ mermaid diagram unavailable"), image_area);
            }
        }
    }
}

/// Reserve stationary chrome around the scrolling transcript, like the live
/// layout. At tiny heights prefer preserving one transcript row over widgets.
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    session: &SessionInfo,
    extras: Option<&PreviewExtras>,
) -> Rect {
    let data = selected_info(session, extras);
    let mut transcript = area;
    if transcript.height > 1 {
        let status_area = Rect {
            y: transcript.y + transcript.height - 1,
            height: 1,
            ..transcript
        };
        let host = (session.server_icon.as_deref() == Some("☁"))
            .then(|| session.server_name.as_deref()?.strip_prefix("cloud: "))
            .flatten();
        frame.render_widget(
            Paragraph::new(
                super::super::ui::input_ui::preview_overscroll_line_for_host(
                    &data, area.width, host,
                ),
            ),
            status_area,
        );
        transcript.height -= 1;
    }
    if transcript.height > 1 {
        let input_area = Rect {
            y: transcript.bottom() - 1,
            height: 1,
            ..transcript
        };
        super::super::ui::input_ui::draw_preview_input(
            frame,
            input_area,
            next_prompt_number(session, extras),
        );
        transcript.height -= 1;
    }
    // The live idle layout keeps a status row immediately above the composer.
    // No live processing state belongs to this selected-session snapshot.
    if transcript.height > 1 {
        transcript.height -= 1;
    }
    transcript
}

/// The same margin measurement and placement engine as the live chat, with
/// fresh private anchors so preview selection cannot move the live HUD.
pub(super) fn draw_widgets(
    frame: &mut Frame,
    area: Rect,
    lines: &[Line<'static>],
    session: &SessionInfo,
    extras: Option<&PreviewExtras>,
    image_regions: &[jcode_tui_messages::ImageRegion],
    scroll: usize,
) {
    let data = selected_info(session, extras);
    let mut margins = super::super::ui::viewport::compute_visible_margins(lines, &[], area, false);
    let visible_end = scroll.saturating_add(area.height as usize);
    for region in image_regions {
        let start = region.abs_line_idx.max(scroll);
        let end = region.end_line.min(visible_end);
        let width = if region.width == 0 {
            area.width
        } else {
            region.width.min(area.width)
        };
        for row in start..end {
            let index = row - scroll;
            if let Some(right) = margins.right_widths.get_mut(index) {
                *right = (*right).min(area.width.saturating_sub(width));
            }
        }
    }
    let placements = super::super::info_widget_layout::calculate_placements_anchored(
        area,
        &margins,
        &data,
        info_widget::is_enabled(),
        &[],
    );
    info_widget::render_all(frame, &placements.visible, &data);
}

#[cfg(test)]
#[path = "preview_chrome_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "preview_composer_tests.rs"]
mod composer_tests;

#[cfg(test)]
#[path = "preview_telemetry_tests.rs"]
mod telemetry_tests;
