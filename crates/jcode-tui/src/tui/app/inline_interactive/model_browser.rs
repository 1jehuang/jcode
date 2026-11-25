use super::*;

pub(super) fn model_picker_provider_hint_from_model_spec(model_spec: &str) -> Option<(&str, &str)> {
    let (provider_hint, bare_model) = model_spec.split_once(':')?;
    let provider_hint = provider_hint.trim();
    let bare_model = bare_model.trim();
    if provider_hint.is_empty() || bare_model.is_empty() {
        return None;
    }

    let normalized = provider_hint.to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "claude"
            | "anthropic"
            | "openai"
            | "copilot"
            | "cursor"
            | "antigravity"
            | "bedrock"
            | "openrouter"
            | "gemini"
    ) || crate::provider_catalog::openai_compatible_profile_by_id(provider_hint).is_some()
    {
        Some((provider_hint, bare_model))
    } else {
        None
    }
}

pub(super) fn model_picker_route_provider_matches_key(
    route_provider_key: Option<&str>,
    route_provider_label: &str,
    desired_provider: &str,
) -> bool {
    jcode_provider_core::model_route_provider_matches_key(
        route_provider_key,
        route_provider_label,
        desired_provider,
    )
}

/// Keep the active route visible even when temporarily blocked. Without an
/// active row, prefer an available saved default, then an available route.
pub(in crate::tui::app) fn model_picker_initial_selection(entries: &[PickerEntry]) -> usize {
    let available =
        |entry: &PickerEntry| entry.active_option().is_some_and(|route| route.available);
    entries
        .iter()
        .position(|entry| entry.is_current)
        .or_else(|| {
            entries
                .iter()
                .position(|entry| entry.is_default && available(entry))
        })
        .or_else(|| entries.iter().position(available))
        .unwrap_or(0)
}

/// Split a model-picker filter into `@provider` scopes and free search text.
/// `@openrouter gpt` keeps OpenRouter routes and fuzzy-matches `gpt`. Only
/// whitespace-separated tokens that start with `@` are scopes, so the direct
/// `/model name@provider` syntax still searches as typed.
pub(crate) fn split_model_picker_filter(filter: &str) -> (Vec<String>, String) {
    let mut scopes = Vec::new();
    let mut search = Vec::new();
    for token in filter.split_whitespace() {
        match token.strip_prefix('@') {
            Some(scope) if !scope.is_empty() => {
                scopes.push(crate::provider::normalize_model_route_provider_label(scope))
            }
            Some(_) => {}
            None => search.push(token),
        }
    }
    (scopes, search.join(" "))
}

/// Provider a route is grouped under for `@provider` filtering and Ctrl+P
/// cycling. OpenRouter routes name their upstream (e.g. `Fireworks`), but the
/// user reaches them through OpenRouter.
pub(super) fn model_route_provider_group(option: &crate::tui::PickerOption) -> String {
    // Azure preserves its own service identity while reusing this legacy transport.
    if crate::provider::ModelRouteApiMethod::parse(&option.api_method).is_openrouter()
        && crate::provider::normalize_model_route_provider_label(&option.provider) != "azureopenai"
    {
        "OpenRouter".to_string()
    } else {
        option.provider.clone()
    }
}

/// Whether a route belongs to every `@provider` scope. Scopes match the
/// provider group the route is reached through, so `@fireworks` never picks an
/// OpenRouter route that merely names Fireworks upstream (selecting it would
/// send the request through OpenRouter); `@openrouter` covers those routes.
pub(super) fn model_route_matches_scopes(
    option: &crate::tui::PickerOption,
    scopes: &[String],
) -> bool {
    let group = model_route_provider_group(option);
    let profile = super::helpers::openai_compatible_profile_id_for_route(option)
        .map(|id| crate::provider::normalize_model_route_provider_label(&id));
    scopes.iter().all(|scope| {
        crate::provider::model_route_provider_labels_match(&group, scope)
            || (crate::provider::normalize_model_route_provider_label(&group) == "azureopenai"
                && scope == "azure")
            || profile.as_ref() == Some(scope)
    })
}

/// The `@provider` scopes currently narrowing a runtime model picker.
pub(super) fn model_picker_scopes(picker: &InlineInteractiveState) -> Vec<String> {
    if picker_is_runtime_model_picker(picker) {
        split_model_picker_filter(&picker.filter).0
    } else {
        Vec::new()
    }
}

/// Next route of `entry` in `direction` (-1 or 1) that the scopes allow,
/// staying put at the ends. Keeps route-column navigation inside the filter
/// the header shows.
pub(super) fn step_scoped_route(
    entry: &crate::tui::PickerEntry,
    scopes: &[String],
    forward: bool,
) -> usize {
    let allowed = |index: &usize| {
        scopes.is_empty() || model_route_matches_scopes(&entry.options[*index], scopes)
    };
    let current = entry.selected_option;
    let next = if forward {
        (current + 1..entry.options.len()).find(allowed)
    } else {
        (0..current).rev().find(allowed)
    };
    next.unwrap_or(current)
}

/// Enter on a model picker with no search text and no explicit row choice
/// opens the browser instead of switching to whatever model is listed first.
pub(in crate::tui::app) fn picker_enter_should_focus_model_browser(
    picker: &InlineInteractiveState,
) -> bool {
    let initial = model_picker_initial_selection(&picker.entries);
    picker_is_runtime_model_picker(picker)
        && picker.selected
            == picker
                .filtered
                .iter()
                .position(|&index| index == initial)
                .unwrap_or(0)
        && split_model_picker_filter(&picker.filter).1.is_empty()
}

/// Distinct provider groups in picker order, used by Ctrl+P.
pub(super) fn model_picker_provider_groups(picker: &InlineInteractiveState) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut groups = Vec::new();
    for entry in &picker.entries {
        if !matches!(entry.action, PickerAction::Model) {
            continue;
        }
        for option in &entry.options {
            let group = model_route_provider_group(option);
            if seen.insert(crate::provider::normalize_model_route_provider_label(
                &group,
            )) {
                groups.push(group);
            }
        }
    }
    groups
}

/// `@scope` token for a provider group, e.g. `GitHub Copilot` -> `@githubcopilot`.
fn provider_scope_token(group: &str) -> String {
    format!(
        "@{}",
        crate::provider::normalize_model_route_provider_label(group)
    )
}

/// Ctrl+P / Ctrl+Shift+P: step the picker's provider scope through every
/// provider group, then back to all providers. Keeps the search text.
pub(super) fn cycle_model_picker_provider(picker: &mut InlineInteractiveState, forward: bool) {
    let groups = model_picker_provider_groups(picker);
    if groups.is_empty() {
        return;
    }
    let (_, search) = split_model_picker_filter(&picker.filter);
    let current = model_picker_active_provider(picker).and_then(|active| {
        groups.iter().position(|group| {
            crate::provider::normalize_model_route_provider_label(group)
                == crate::provider::normalize_model_route_provider_label(&active)
        })
    });
    // Positions 0..len are providers, len is "all providers".
    let slots = groups.len() + 1;
    let at = current.unwrap_or(groups.len());
    let next = if forward {
        (at + 1) % slots
    } else {
        (at + slots - 1) % slots
    };
    picker.filter = match groups.get(next) {
        Some(group) if search.is_empty() => format!("{} ", provider_scope_token(group)),
        Some(group) => format!("{} {}", provider_scope_token(group), search),
        None => search,
    };
    picker.selected = 0;
    App::apply_inline_interactive_filter(picker);
}

/// Provider group named by the picker's single `@scope`, for display.
pub(crate) fn model_picker_active_provider(picker: &InlineInteractiveState) -> Option<String> {
    let (scopes, _) = split_model_picker_filter(&picker.filter);
    let [scope] = scopes.as_slice() else {
        return (!scopes.is_empty()).then(|| scopes.join(", "));
    };
    Some(
        picker
            .entries
            .iter()
            .flat_map(|entry| &entry.options)
            .find(|route| model_route_matches_scopes(route, &scopes))
            .map(model_route_provider_group)
            .unwrap_or_else(|| scope.clone()),
    )
}

impl InlineInteractiveState {
    pub fn choose_route(&mut self, index: usize, option: usize) {
        if let Some(entry) = self.entries.get_mut(index)
            && option < entry.options.len()
            && option != entry.selected_option
        {
            entry.selected_option = option;
            self.scoped_route_restore.retain(|(i, _)| *i != index);
        }
    }
}

impl App {
    pub(super) fn handle_model_browser_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        let Some(picker) = self
            .inline_interactive_state
            .as_mut()
            .filter(|p| picker_is_runtime_model_picker(p))
        else {
            return false;
        };
        if modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'p') {
            cycle_model_picker_provider(picker, !modifiers.contains(KeyModifiers::SHIFT));
            picker.column = 0;
            return true;
        }
        if matches!(
            code,
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
        ) {
            let last = picker.filtered.len().saturating_sub(1);
            picker.selected = match code {
                KeyCode::Home => 0,
                KeyCode::End => last,
                KeyCode::PageUp => picker.selected.saturating_sub(5),
                _ => (picker.selected + 5).min(last),
            };
            return true;
        }
        if code == KeyCode::Backspace || (code == KeyCode::Esc && !picker.filter.is_empty()) {
            let selected = picker.selected_entry_index();
            if code == KeyCode::Esc {
                picker.filter.clear();
            } else {
                picker.filter.pop();
            }
            Self::apply_inline_interactive_filter(picker);
            if let Some(position) =
                selected.and_then(|index| picker.filtered.iter().position(|&i| i == index))
            {
                picker.selected = position;
            }
            return true;
        }
        if modifiers.contains(KeyModifiers::CONTROL)
            && key_char_eq_ignore_ascii_case(code, 'o')
            && let Some(entry) = picker.selected_entry()
            && let Some(route) = entry.active_option().filter(|r| !r.available)
        {
            let notice = format!("{} unavailable - {}", entry.name, route.detail);
            self.set_status_notice(notice);
            return true;
        }
        false
    }
}

pub(super) fn key_char_eq_ignore_ascii_case(code: KeyCode, expected: char) -> bool {
    matches!(code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&expected))
}

impl App {
    /// When a runtime model-picker preview is visible, route its favorite/default
    /// hotkeys to the focused picker handler. Returns true if the key was consumed.
    pub(in crate::tui::app) fn model_picker_preview_hotkey(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let active = self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.preview && picker_is_runtime_model_picker(picker))
            .unwrap_or(false);
        if !active {
            return Ok(false);
        }
        // Preserve tmux/readline Ctrl+B, Ctrl+F and Alt+F in the composer;
        // Ctrl+O saves defaults, Ctrl+N favorites, Shift+Tab cycles favorites.
        let is_default =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'o');
        let is_favorite =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'n');
        if is_default || is_favorite {
            self.handle_inline_interactive_key(code, modifiers)?;
            return Ok(true);
        }
        if modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'p') {
            if let Some(picker) = self.inline_interactive_state.as_mut() {
                picker.preview = false;
                picker.column = 0;
            }
            self.input.clear();
            self.cursor_pos = 0;
            self.handle_inline_interactive_key(code, modifiers)?;
            return Ok(true);
        }
        Ok(false)
    }
}

pub(super) fn restore_model_browser_after_refresh(
    picker: &mut InlineInteractiveState,
    previous: &InlineInteractiveState,
) {
    let originals: HashMap<usize, usize> = previous.scoped_route_restore.iter().copied().collect();
    let choices: HashSet<(&str, &str, &str)> = previous
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .options
                .get(
                    originals
                        .get(&index)
                        .copied()
                        .unwrap_or(entry.selected_option),
                )
                .map(|route| {
                    (
                        entry.name.as_str(),
                        route.provider.as_str(),
                        route.api_method.as_str(),
                    )
                })
        })
        .collect();
    for entry in &mut picker.entries {
        if let Some(option) = entry.options.iter().position(|r| {
            choices.contains(&(
                entry.name.as_str(),
                r.provider.as_str(),
                r.api_method.as_str(),
            ))
        }) {
            entry.selected_option = option;
        }
    }
    App::apply_inline_interactive_filter(picker);
    if let Some(selected) = previous.selected_entry()
        && let Some(route) = selected.active_option()
        && let Some(position) = picker.filtered.iter().position(|&i| {
            picker.entries[i].name == selected.name
                && picker.entries[i].active_option().is_some_and(|r| {
                    r.provider == route.provider && r.api_method == route.api_method
                })
        })
    {
        picker.selected = position;
    }
}

impl InlineInteractiveState {
    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let entries_bytes: usize = self
            .entries
            .iter()
            .map(crate::tui::estimate_picker_entry_bytes)
            .sum();
        let effort_step_bytes = self
            .effort_step
            .as_ref()
            .map(|step| {
                crate::tui::estimate_picker_entry_bytes(&step.model)
                    + step.parent.debug_memory_profile()["total_estimate_bytes"]
                        .as_u64()
                        .unwrap_or(0) as usize
            })
            .unwrap_or(0);
        let filtered_bytes = self.filtered.capacity() * std::mem::size_of::<usize>();
        let filter_bytes = self.filter.capacity();
        let scoped_routes_bytes =
            self.scoped_route_restore.capacity() * std::mem::size_of::<(usize, usize)>();
        serde_json::json!({
            "entries_count": self.entries.len(),
            "filtered_count": self.filtered.len(),
            "entries_bytes": entries_bytes,
            "filtered_bytes": filtered_bytes,
            "filter_bytes": filter_bytes,
            "scoped_routes_bytes": scoped_routes_bytes,
            "effort_step_bytes": effort_step_bytes,
            "total_estimate_bytes": entries_bytes + filtered_bytes + filter_bytes + scoped_routes_bytes + effort_step_bytes,
        })
    }
}
