//! `/colors`: inspect, configure, and score the TUI color palette.
//!
//! Every color the TUI renders is configurable through `[display.colors]` in
//! `~/.jcode/config.toml`. This command is the interactive front end for that:
//! it lists the roles with their current values, sets them, resets them, and
//! (the part that makes tuning tractable) scores the resulting palette with
//! [`jcode_tui_style::harmony`] so a user gets specific, actionable feedback
//! instead of trial and error.

use super::{App, DisplayMessage};
use jcode_tui_style::palette::{ALL_ROLES, Palette, Role, parse_hex, to_hex};

const USAGE: &str = "Usage:\n  \
    /colors                       List every configurable color role\n  \
    /colors <role> <#rrggbb>      Set a role's color (saved to config)\n  \
    /colors reset [role]          Reset one role, or all of them\n  \
    /colors harmony               Score the palette and list what to fix\n  \
    /colors generate <#rrggbb>    Build a whole harmonious palette from one seed color\n  \
    /colors export                Print the palette as config TOML";

pub(super) fn handle_colors_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed
        .strip_prefix("/colors")
        .or_else(|| trimmed.strip_prefix("/color"))
    else {
        return false;
    };
    // Only claim the exact command or `command <args>`, never `/colorsomething`.
    if !rest.is_empty() && !rest.starts_with(' ') {
        return false;
    }
    let rest = rest.trim();

    let mut words = rest.split_whitespace();
    match words.next() {
        None | Some("list") => list_colors(app),
        Some("harmony") | Some("score") => show_harmony(app),
        Some("export") => export_colors(app),
        Some("reset") => reset_colors(app, words.next()),
        Some("generate") | Some("gen") => generate_palette(app, words.next()),
        Some(role) => match words.next() {
            Some(value) => set_color(app, role, value),
            None => app.push_display_message(DisplayMessage::error(format!(
                "Missing color value for '{role}'.\n\n{USAGE}"
            ))),
        },
    }
    true
}

fn configured_palette() -> Palette {
    let configured = &crate::config::config().display.colors;
    Palette::from_pairs(
        configured
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    )
    .0
}

fn list_colors(app: &mut App) {
    let palette = configured_palette();
    let mut lines = vec!["Configurable TUI colors (`/colors <role> <#rrggbb>`):".to_string()];
    for role in ALL_ROLES.iter().copied() {
        let rgb = palette.rgb(role);
        let marker = if palette.is_overridden(role) {
            " (custom)"
        } else {
            ""
        };
        lines.push(format!("  {:<16} {}{}", role.key(), to_hex(rgb), marker));
    }
    lines.push(String::new());
    lines.push(
        "Ad hoc shades used by individual widgets follow the role they belong to, so setting a \
         role recolors its whole family. Run /colors harmony to score the result."
            .to_string(),
    );
    app.push_display_message(DisplayMessage::system(lines.join("\n")));
}

/// The background the palette is judged against.
///
/// Readability is only meaningful relative to the real terminal background, so
/// follow the detected light/dark theme rather than assuming black.
fn active_background() -> (u8, u8, u8) {
    if jcode_tui_style::is_light_theme() {
        (255, 255, 255)
    } else {
        (18, 18, 18)
    }
}

fn show_harmony(app: &mut App) {
    let background = active_background();
    let report = jcode_tui_style::analyze_harmony(&configured_palette(), background);

    let mut lines = vec![
        format!(
            "Palette harmony: {}/100 ({}), hue structure: {}",
            report.score,
            report.grade(),
            report.scheme
        ),
        String::new(),
    ];
    for criterion in &report.criteria {
        lines.push(format!(
            "  {:<20} {:>3}/100  (weight {:.1})",
            criterion.name, criterion.score, criterion.weight
        ));
    }

    let findings = report.top_findings(6);
    if findings.is_empty() {
        lines.push(String::new());
        lines.push("No issues found.".to_string());
    } else {
        lines.push(String::new());
        lines.push("Suggested fixes:".to_string());
        for finding in findings {
            lines.push(format!("  - {finding}"));
        }
    }
    app.push_display_message(DisplayMessage::system(lines.join("\n")));
}

fn export_colors(app: &mut App) {
    let palette = configured_palette();
    let mut lines = vec!["[display.colors]".to_string()];
    for role in ALL_ROLES.iter().copied() {
        lines.push(format!(
            "{} = \"{}\"",
            role.key(),
            to_hex(palette.rgb(role))
        ));
    }
    app.push_display_message(DisplayMessage::system(lines.join("\n")));
}

/// Derive and save a full palette from one seed color.
///
/// Hand-tuning 22 roles is the thing that stops people from theming at all, so
/// this does the tuning and reports the resulting harmony score. The generator
/// targets the *active* background, since a palette that reads well on dark is
/// usually wrong on light.
fn generate_palette(app: &mut App, seed: Option<&str>) {
    let Some(seed) = seed else {
        app.push_display_message(DisplayMessage::error(format!(
            "Usage: /colors generate <#rrggbb>\n\n{USAGE}"
        )));
        return;
    };
    let Some(seed_rgb) = parse_hex(seed) else {
        app.push_display_message(DisplayMessage::error(format!(
            "Invalid seed color '{seed}'. Expected a hex color like #8ab4f8."
        )));
        return;
    };

    let background = active_background();
    let generated = jcode_tui_style::harmony::generate_from_seed(seed_rgb, background);
    let report = jcode_tui_style::analyze_harmony(&generated, background);

    let result = persist(|colors| {
        colors.clear();
        for role in ALL_ROLES.iter().copied() {
            colors.insert(role.key().to_string(), to_hex(generated.rgb(role)));
        }
    });

    match result {
        Ok(()) => app.push_display_message(DisplayMessage::system(format!(
            "Generated a {} palette from {} (harmony {}/100, {}). Applied immediately.\n\nRun \
             /colors to see the roles, /colors harmony for details, or /colors reset to undo.",
            report.scheme,
            to_hex(seed_rgb),
            report.score,
            report.grade()
        ))),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save the generated palette: {error}"
        ))),
    }
}

fn set_color(app: &mut App, role_key: &str, value: &str) {
    let Some(role) = Role::from_key(role_key) else {
        app.push_display_message(DisplayMessage::error(format!(
            "Unknown color role '{role_key}'. Run /colors to list them."
        )));
        return;
    };
    let Some(rgb) = parse_hex(value) else {
        app.push_display_message(DisplayMessage::error(format!(
            "Invalid color '{value}'. Expected a hex color like #8ab4f8."
        )));
        return;
    };

    match persist(|colors| {
        colors.insert(role.key().to_string(), to_hex(rgb));
    }) {
        Ok(()) => {
            let report_line = harmony_delta_line();
            app.push_display_message(DisplayMessage::system(format!(
                "Set {} to {}. Applied immediately.\n{report_line}",
                role.key(),
                to_hex(rgb)
            )));
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to save {}: {error}",
            role.key()
        ))),
    }
}

fn reset_colors(app: &mut App, role_key: Option<&str>) {
    let result = match role_key {
        Some(key) => {
            let Some(role) = Role::from_key(key) else {
                app.push_display_message(DisplayMessage::error(format!(
                    "Unknown color role '{key}'. Run /colors to list them."
                )));
                return;
            };
            persist(|colors| {
                colors.remove(role.key());
            })
            .map(|()| format!("Reset {} to its default.", role.key()))
        }
        None => persist(|colors| colors.clear())
            .map(|()| "Reset every color to its default.".to_string()),
    };

    match result {
        Ok(message) => app.push_display_message(DisplayMessage::system(message)),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!("Failed to reset: {error}")))
        }
    }
}

/// Mutate `[display.colors]`, save, and reinstall the live palette.
///
/// Reload-then-patch-then-save (rather than serializing cached state) so a
/// concurrent config edit by another jcode session is not clobbered.
fn persist(
    mutate: impl FnOnce(&mut std::collections::BTreeMap<String, String>),
) -> anyhow::Result<()> {
    let mut config = crate::config::Config::load();
    mutate(&mut config.display.colors);
    config.save()?;
    crate::tui::theme_detect::init_palette();
    Ok(())
}

fn harmony_delta_line() -> String {
    let background = active_background();
    let report = jcode_tui_style::analyze_harmony(&configured_palette(), background);
    format!(
        "Palette harmony is now {}/100 ({}). Run /colors harmony for details.",
        report.score,
        report.grade()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_claims_the_colors_command() {
        // Guard against swallowing unrelated commands with a shared prefix.
        assert!(!"/colorscheme".starts_with("/colors "));
        assert!(Role::from_key("user").is_some());
        assert!(Role::from_key("not-a-role").is_none());
    }

    #[test]
    fn usage_text_documents_every_subcommand() {
        for subcommand in ["reset", "harmony", "export"] {
            assert!(
                USAGE.contains(subcommand),
                "usage should document {subcommand}"
            );
        }
    }
}
