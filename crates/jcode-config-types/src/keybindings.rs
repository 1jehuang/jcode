//! Per-platform keybinding default registry, provenance metadata, and a
//! validation/"check" layer.
//!
//! Historically jcode used a single shared list of default keybindings for
//! every platform, and macOS support was implemented purely as a runtime
//! translation layer (mapping Option-inserted Unicode characters and `Cmd`
//! fallbacks back onto the shared `Alt`/`Ctrl` defaults).
//!
//! This module splits the defaults into two explicit lists, one for macOS and
//! one for everything else (Windows/Linux), and records *how each default was
//! chosen* so we can tell the difference between:
//!
//! * a binding a developer explicitly decided should be the default on that
//!   platform ([`KeybindingProvenance::Dev`]),
//! * a binding an AI agent picked ([`KeybindingProvenance::Ai`]), and
//! * a binding that was derived programmatically, e.g. auto-translated from the
//!   other platform's binding ([`KeybindingProvenance::Automatic`]).
//!
//! The [`validate_keybinding_defaults`] check layer surfaces warnings when the
//! two lists drift apart: a binding bound on one platform but missing on the
//! other, or an auto-translated default that nobody has confirmed as an
//! explicit choice yet.

/// Which platform family a default keybinding targets.
///
/// macOS terminals encode several modifier combinations differently from
/// Windows/Linux, so the two families get independent default lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindingPlatform {
    /// Apple platforms (macOS).
    MacOs,
    /// Windows, Linux, and other non-macOS platforms.
    Other,
}

impl KeybindingPlatform {
    /// The platform this binary was compiled for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            KeybindingPlatform::MacOs
        } else {
            KeybindingPlatform::Other
        }
    }

    /// The "opposite" platform family. Useful for asymmetry diagnostics.
    pub const fn counterpart(self) -> Self {
        match self {
            KeybindingPlatform::MacOs => KeybindingPlatform::Other,
            KeybindingPlatform::Other => KeybindingPlatform::MacOs,
        }
    }

    /// Human-readable label.
    pub const fn label(self) -> &'static str {
        match self {
            KeybindingPlatform::MacOs => "macOS",
            KeybindingPlatform::Other => "Windows/Linux",
        }
    }
}

/// How a default keybinding came to be chosen.
///
/// This is the distinction the validation layer cares about: an
/// auto-translation is *not* the same as an explicit decision that a binding is
/// the right default for a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindingProvenance {
    /// Explicitly chosen by a human developer for this platform.
    Dev,
    /// Chosen by an AI agent.
    Ai,
    /// Derived programmatically (e.g. auto-translated from the other platform's
    /// binding) rather than explicitly decided for this platform.
    Automatic,
}

impl KeybindingProvenance {
    /// Short label for diagnostics/UI.
    pub const fn label(self) -> &'static str {
        match self {
            KeybindingProvenance::Dev => "dev-chosen",
            KeybindingProvenance::Ai => "ai-chosen",
            KeybindingProvenance::Automatic => "automatic",
        }
    }

    /// Whether a human or AI explicitly decided on this binding for the
    /// platform (as opposed to it being auto-derived).
    pub const fn is_explicit(self) -> bool {
        matches!(self, KeybindingProvenance::Dev | KeybindingProvenance::Ai)
    }
}

/// A platform-specific default value plus how it was chosen.
#[derive(Debug, Clone, Copy)]
pub struct PlatformDefault {
    /// The default binding string (e.g. `"alt+h"`). Empty / `"none"` means the
    /// action has no default on this platform.
    pub binding: &'static str,
    /// How this default was chosen.
    pub provenance: KeybindingProvenance,
}

impl PlatformDefault {
    /// A default explicitly chosen by a developer.
    pub const fn dev(binding: &'static str) -> Self {
        Self {
            binding,
            provenance: KeybindingProvenance::Dev,
        }
    }

    /// A default chosen by an AI agent.
    pub const fn ai(binding: &'static str) -> Self {
        Self {
            binding,
            provenance: KeybindingProvenance::Ai,
        }
    }

    /// A default derived programmatically (e.g. auto-translated).
    pub const fn auto(binding: &'static str) -> Self {
        Self {
            binding,
            provenance: KeybindingProvenance::Automatic,
        }
    }

    /// An explicitly "no default on this platform" marker. Used to acknowledge
    /// that the asymmetry is intentional and silence the asymmetry warning.
    pub const fn unbound(provenance: KeybindingProvenance) -> Self {
        Self {
            binding: "",
            provenance,
        }
    }

    /// Whether this default actually binds anything.
    pub fn is_bound(&self) -> bool {
        is_bound_value(self.binding)
    }
}

/// One keybinding action and its defaults across platforms.
#[derive(Debug, Clone, Copy)]
pub struct KeybindingDefault {
    /// Stable identifier. Matches the corresponding `KeybindingsConfig` field.
    pub id: &'static str,
    /// Human-readable description of what the action does.
    pub description: &'static str,
    /// Default for macOS.
    pub macos: PlatformDefault,
    /// Default for Windows/Linux.
    pub other: PlatformDefault,
}

impl KeybindingDefault {
    /// The platform-specific default for `platform`.
    pub const fn platform(&self, platform: KeybindingPlatform) -> &PlatformDefault {
        match platform {
            KeybindingPlatform::MacOs => &self.macos,
            KeybindingPlatform::Other => &self.other,
        }
    }

    /// The default binding string for `platform`.
    pub const fn binding_for(&self, platform: KeybindingPlatform) -> &'static str {
        self.platform(platform).binding
    }
}

/// Returns `true` when `value` actually binds a key (non-empty and not an
/// explicit disable token).
pub fn is_bound_value(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    !matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    )
}

/// The single source of truth for default keybindings, per platform.
///
/// Baseline note: these bindings were historically a single shared list that a
/// developer chose to apply to every platform, so both columns start as
/// [`KeybindingProvenance::Dev`]. When you add a binding for only one platform,
/// set the counterpart to [`PlatformDefault::auto`] (auto-translated) or
/// [`PlatformDefault::unbound`]; the [`validate_keybinding_defaults`] check
/// layer will then warn you to confirm or fix it.
pub const KEYBINDING_DEFAULTS: &[KeybindingDefault] = &[
    KeybindingDefault {
        id: "scroll_up",
        description: "Scroll the transcript up one step",
        // Shift+J/K scroll incrementally; the un-shifted Ctrl+J/K move by prompt.
        macos: PlatformDefault::dev("ctrl+shift+k"),
        other: PlatformDefault::dev("ctrl+shift+k"),
    },
    KeybindingDefault {
        id: "scroll_down",
        description: "Scroll the transcript down one step",
        macos: PlatformDefault::dev("ctrl+shift+j"),
        other: PlatformDefault::dev("ctrl+shift+j"),
    },
    KeybindingDefault {
        id: "scroll_page_up",
        description: "Scroll the transcript up one page",
        macos: PlatformDefault::dev("alt+u"),
        other: PlatformDefault::dev("alt+u"),
    },
    KeybindingDefault {
        id: "scroll_page_down",
        description: "Scroll the transcript down one page",
        macos: PlatformDefault::dev("alt+d"),
        other: PlatformDefault::dev("alt+d"),
    },
    KeybindingDefault {
        id: "model_switch_next",
        description: "Switch to the next model",
        macos: PlatformDefault::dev("ctrl+tab"),
        other: PlatformDefault::dev("ctrl+tab"),
    },
    KeybindingDefault {
        id: "model_switch_prev",
        description: "Switch to the previous model",
        macos: PlatformDefault::dev("ctrl+shift+tab"),
        other: PlatformDefault::dev("ctrl+shift+tab"),
    },
    KeybindingDefault {
        id: "fallback_switch",
        description: "Accept the post-error fallback offer (switch model/auth-method and resend)",
        macos: PlatformDefault::dev("ctrl+y"),
        other: PlatformDefault::dev("ctrl+y"),
    },
    KeybindingDefault {
        id: "effort_increase",
        description: "Increase reasoning effort",
        // macOS uses Cmd+Right so Option+Right stays free for word navigation.
        macos: PlatformDefault::dev("cmd+right"),
        other: PlatformDefault::dev("alt+right"),
    },
    KeybindingDefault {
        id: "effort_decrease",
        description: "Decrease reasoning effort",
        // macOS uses Cmd+Left so Option+Left stays free for word navigation.
        macos: PlatformDefault::dev("cmd+left"),
        other: PlatformDefault::dev("alt+left"),
    },
    KeybindingDefault {
        id: "centered_toggle",
        description: "Toggle centered mode",
        macos: PlatformDefault::dev("alt+c"),
        other: PlatformDefault::dev("alt+c"),
    },
    KeybindingDefault {
        id: "scroll_prompt_up",
        description: "Jump to the previous user prompt",
        // Ctrl+K moves by prompt; Ctrl+[ stays as an equivalent fallback. The
        // built-in prompt_jump handler also accepts Cmd/Option+K best-effort.
        macos: PlatformDefault::dev("ctrl+k"),
        other: PlatformDefault::dev("ctrl+k"),
    },
    KeybindingDefault {
        id: "scroll_prompt_down",
        description: "Jump to the next user prompt",
        macos: PlatformDefault::dev("ctrl+j"),
        other: PlatformDefault::dev("ctrl+j"),
    },
    KeybindingDefault {
        id: "scroll_bookmark",
        description: "Toggle the scroll bookmark",
        macos: PlatformDefault::dev("ctrl+g"),
        other: PlatformDefault::dev("ctrl+g"),
    },
    KeybindingDefault {
        id: "auto_poke_toggle",
        description: "Toggle auto-poke",
        macos: PlatformDefault::dev("ctrl+p"),
        other: PlatformDefault::dev("ctrl+p"),
    },
    KeybindingDefault {
        id: "scroll_up_fallback",
        description: "Optional fallback scroll-up binding",
        // Left unbound on every platform by default: on macOS Cmd+K is reserved
        // for prompt navigation.
        macos: PlatformDefault::unbound(KeybindingProvenance::Dev),
        other: PlatformDefault::unbound(KeybindingProvenance::Dev),
    },
    KeybindingDefault {
        id: "scroll_down_fallback",
        description: "Optional fallback scroll-down binding",
        macos: PlatformDefault::unbound(KeybindingProvenance::Dev),
        other: PlatformDefault::unbound(KeybindingProvenance::Dev),
    },
    KeybindingDefault {
        id: "workspace_left",
        description: "Move to the workspace on the left",
        macos: PlatformDefault::dev("alt+h"),
        other: PlatformDefault::dev("alt+h"),
    },
    KeybindingDefault {
        id: "workspace_down",
        description: "Move to the workspace below",
        macos: PlatformDefault::dev("alt+j"),
        other: PlatformDefault::dev("alt+j"),
    },
    KeybindingDefault {
        id: "workspace_up",
        description: "Move to the workspace above",
        macos: PlatformDefault::dev("alt+k"),
        other: PlatformDefault::dev("alt+k"),
    },
    KeybindingDefault {
        id: "workspace_right",
        description: "Move to the workspace on the right",
        macos: PlatformDefault::dev("alt+l"),
        other: PlatformDefault::dev("alt+l"),
    },
    KeybindingDefault {
        id: "new_terminal",
        description: "Spawn a fresh jcode session in a new terminal window",
        // Companion to the system-wide launch hotkey (Cmd+; / Alt+;): the
        // shifted variant spawns a new session from inside jcode, reusing the
        // current session's working directory. Note: many macOS terminals
        // intercept Cmd combos, so this only fires when the terminal forwards
        // the key to the TUI.
        macos: PlatformDefault::dev("cmd+shift+;"),
        other: PlatformDefault::dev("alt+shift+;"),
    },
    KeybindingDefault {
        id: "open_resume",
        description: "Open the /resume session picker",
        // Cmd+B is free in jcode on macOS. On Windows/Linux, Ctrl+R is already
        // taken by session recovery, so use Alt+R there.
        macos: PlatformDefault::dev("cmd+b"),
        other: PlatformDefault::dev("alt+r"),
    },
];

/// Look up a keybinding action by id.
pub fn keybinding_default(id: &str) -> Option<&'static KeybindingDefault> {
    KEYBINDING_DEFAULTS.iter().find(|entry| entry.id == id)
}

/// The default binding string for `id` on `platform`, or `None` if `id` is not
/// a known action.
pub fn default_binding(id: &str, platform: KeybindingPlatform) -> Option<&'static str> {
    keybinding_default(id).map(|entry| entry.binding_for(platform))
}

/// The default binding string for `id` on the current platform, falling back to
/// `fallback` for unknown ids (which should never happen for real fields).
pub fn default_binding_or(id: &str, fallback: &'static str) -> String {
    default_binding(id, KeybindingPlatform::current())
        .unwrap_or(fallback)
        .to_string()
}

/// The kind of problem the check layer detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindingIssueKind {
    /// Bound on one platform but missing on the other.
    Asymmetric {
        /// The platform that *does* have a binding.
        bound_on: KeybindingPlatform,
    },
    /// An auto-translated default that hasn't been confirmed as an explicit
    /// choice, while the counterpart platform *was* chosen explicitly.
    UnconfirmedAutomatic {
        /// The platform whose default is auto-derived.
        platform: KeybindingPlatform,
    },
}

/// A single diagnostic emitted by [`validate_keybinding_defaults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingIssue {
    /// The action id this issue concerns.
    pub id: &'static str,
    /// What kind of problem was found.
    pub kind: KeybindingIssueKind,
    /// A human-readable warning message.
    pub message: String,
}

/// The check layer.
///
/// Compares the macOS and Other default lists and returns a warning for every
/// place they drift apart:
///
/// * **Asymmetry** - a binding exists for one platform but not the other. This
///   catches the case where you add a binding on Linux but forget macOS (or
///   vice-versa).
/// * **Unconfirmed auto-translation** - one platform's default is
///   [`KeybindingProvenance::Automatic`] while the other was chosen explicitly
///   ([`Dev`](KeybindingProvenance::Dev)/[`Ai`](KeybindingProvenance::Ai)). The
///   binding works, but nobody has confirmed it is the *right* default for that
///   platform, so it is flagged for review.
pub fn validate_keybinding_defaults() -> Vec<KeybindingIssue> {
    validate_defaults(KEYBINDING_DEFAULTS)
}

fn validate_defaults(defaults: &[KeybindingDefault]) -> Vec<KeybindingIssue> {
    let mut issues = Vec::new();

    for entry in defaults {
        let macos_bound = entry.macos.is_bound();
        let other_bound = entry.other.is_bound();

        match (macos_bound, other_bound) {
            (true, false) => issues.push(asymmetry_issue(entry, KeybindingPlatform::MacOs)),
            (false, true) => issues.push(asymmetry_issue(entry, KeybindingPlatform::Other)),
            (true, true) => {
                // Both bound: flag an auto-translated default whose counterpart
                // was an explicit choice, so it can be confirmed or corrected.
                for platform in [KeybindingPlatform::MacOs, KeybindingPlatform::Other] {
                    let here = entry.platform(platform);
                    let there = entry.platform(platform.counterpart());
                    if here.provenance == KeybindingProvenance::Automatic
                        && there.provenance.is_explicit()
                    {
                        issues.push(unconfirmed_automatic_issue(entry, platform));
                    }
                }
            }
            (false, false) => {}
        }
    }

    issues
}

fn asymmetry_issue(entry: &KeybindingDefault, bound_on: KeybindingPlatform) -> KeybindingIssue {
    let missing_on = bound_on.counterpart();
    let bound = entry.platform(bound_on);
    KeybindingIssue {
        id: entry.id,
        kind: KeybindingIssueKind::Asymmetric { bound_on },
        message: format!(
            "`{id}` ({desc}) is bound on {bound_label} (`{binding}`, {prov}) but has no default on {missing_label}. \
Add an explicit binding for {missing_label}, mark it auto-translated, or mark it intentionally unbound.",
            id = entry.id,
            desc = entry.description,
            bound_label = bound_on.label(),
            binding = bound.binding,
            prov = bound.provenance.label(),
            missing_label = missing_on.label(),
        ),
    }
}

fn unconfirmed_automatic_issue(
    entry: &KeybindingDefault,
    platform: KeybindingPlatform,
) -> KeybindingIssue {
    let here = entry.platform(platform);
    let there = entry.platform(platform.counterpart());
    KeybindingIssue {
        id: entry.id,
        kind: KeybindingIssueKind::UnconfirmedAutomatic { platform },
        message: format!(
            "`{id}` ({desc}) default for {here_label} (`{here_binding}`) is auto-translated from {there_label}'s `{there_binding}` ({there_prov}). \
Confirm it as the intended default for {here_label} (dev/ai) or adjust it.",
            id = entry.id,
            desc = entry.description,
            here_label = platform.label(),
            here_binding = here.binding,
            there_label = platform.counterpart().label(),
            there_binding = there.binding,
            there_prov = there.provenance.label(),
        ),
    }
}

/// A human-readable report of every default binding and its provenance, for both
/// platforms. Useful for `/config`-style surfaces and debugging.
pub fn keybinding_defaults_report() -> String {
    let mut out = String::new();
    out.push_str("Keybinding defaults (macOS | Windows/Linux):\n");
    for entry in KEYBINDING_DEFAULTS {
        out.push_str(&format!(
            "  {id:<20} {mac:<16} ({mac_prov})  |  {other:<16} ({other_prov})\n",
            id = entry.id,
            mac = display_binding(entry.macos.binding),
            mac_prov = entry.macos.provenance.label(),
            other = display_binding(entry.other.binding),
            other_prov = entry.other.provenance.label(),
        ));
    }

    let issues = validate_keybinding_defaults();
    if issues.is_empty() {
        out.push_str("\nNo keybinding default asymmetries detected.\n");
    } else {
        out.push_str(&format!("\n{} warning(s):\n", issues.len()));
        for issue in issues {
            out.push_str(&format!("  - {}\n", issue.message));
        }
    }
    out
}

fn display_binding(binding: &str) -> String {
    if is_bound_value(binding) {
        binding.to_string()
    } else {
        "(unbound)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_id_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in KEYBINDING_DEFAULTS {
            assert!(
                seen.insert(entry.id),
                "duplicate keybinding id: {}",
                entry.id
            );
        }
    }

    #[test]
    fn baseline_defaults_have_no_warnings() {
        // The shipped baseline is a clean slate: matched, explicit choices.
        let issues = validate_keybinding_defaults();
        assert!(
            issues.is_empty(),
            "expected no baseline warnings, got: {:#?}",
            issues
        );
    }

    #[test]
    fn detects_asymmetric_binding() {
        let defaults = &[KeybindingDefault {
            id: "example",
            description: "Example action",
            macos: PlatformDefault::dev("alt+x"),
            other: PlatformDefault::unbound(KeybindingProvenance::Automatic),
        }];
        let issues = validate_defaults(defaults);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].kind,
            KeybindingIssueKind::Asymmetric {
                bound_on: KeybindingPlatform::MacOs
            }
        );
    }

    #[test]
    fn detects_asymmetric_binding_other_direction() {
        // Add a binding on Linux/Windows but forget macOS -> warn.
        let defaults = &[KeybindingDefault {
            id: "example",
            description: "Example action",
            macos: PlatformDefault::unbound(KeybindingProvenance::Dev),
            other: PlatformDefault::dev("ctrl+x"),
        }];
        let issues = validate_defaults(defaults);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].kind,
            KeybindingIssueKind::Asymmetric {
                bound_on: KeybindingPlatform::Other
            }
        );
    }

    #[test]
    fn detects_unconfirmed_auto_translation() {
        // Dev chose a Linux binding; macOS got an auto-translation that hasn't
        // been confirmed -> warn so the dev can promote or adjust it.
        let defaults = &[KeybindingDefault {
            id: "example",
            description: "Example action",
            macos: PlatformDefault::auto("alt+x"),
            other: PlatformDefault::dev("alt+x"),
        }];
        let issues = validate_defaults(defaults);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].kind,
            KeybindingIssueKind::UnconfirmedAutomatic {
                platform: KeybindingPlatform::MacOs
            }
        );
    }

    #[test]
    fn confirmed_explicit_per_platform_bindings_are_quiet() {
        // Different bindings per platform, both explicit -> no warning.
        let defaults = &[KeybindingDefault {
            id: "example",
            description: "Example action",
            macos: PlatformDefault::dev("cmd+x"),
            other: PlatformDefault::ai("ctrl+x"),
        }];
        assert!(validate_defaults(defaults).is_empty());
    }

    #[test]
    fn both_automatic_is_not_flagged_as_unconfirmed() {
        // If neither side is an explicit choice, there is no explicit
        // counterpart to confirm against, so stay quiet.
        let defaults = &[KeybindingDefault {
            id: "example",
            description: "Example action",
            macos: PlatformDefault::auto("alt+x"),
            other: PlatformDefault::auto("alt+x"),
        }];
        assert!(validate_defaults(defaults).is_empty());
    }

    #[test]
    fn default_binding_lookup_matches_platform() {
        assert_eq!(
            default_binding("workspace_left", KeybindingPlatform::MacOs),
            Some("alt+h")
        );
        assert_eq!(
            default_binding("scroll_up_fallback", KeybindingPlatform::Other),
            Some("")
        );
        assert_eq!(
            default_binding("does_not_exist", KeybindingPlatform::Other),
            None
        );
    }
}
