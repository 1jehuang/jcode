use super::*;

pub(super) fn model_picker_key_without_effort(key: &str) -> String {
    match key.rsplit_once('\u{1f}') {
        Some((route, _effort)) => format!("{route}\u{1f}"),
        None => key.to_string(),
    }
}

pub(super) fn fold_model_picker_usage_efforts(store: &mut ModelPickerUsageStore) {
    let mut folded: HashMap<String, ModelPickerUsageEntry> = HashMap::new();
    for (key, entry) in std::mem::take(&mut store.selections) {
        let merged = folded
            .entry(model_picker_key_without_effort(&key))
            .or_default();
        merged.count = merged.count.saturating_add(entry.count);
        merged.last_selected_unix_secs = merged
            .last_selected_unix_secs
            .max(entry.last_selected_unix_secs);
    }
    store.selections = folded;
}

pub(in crate::tui::app) fn model_route_effort_levels(
    model: &str,
    route: &PickerOption,
) -> Vec<&'static str> {
    if !route_supports_reasoning_effort(&route.api_method) {
        return Vec::new();
    }
    inferred_reasoning_efforts(Some(&route.api_method), Some(model))
        .into_iter()
        .filter(|effort| !crate::prompt::is_swarm_mode_effort(effort))
        .collect()
}

pub(super) fn reasoning_effort_picker_label(effort: &str) -> &str {
    match effort {
        "medium" => "med",
        other => other,
    }
}

pub(super) fn saved_default_reasoning_effort(model: &str, route: &PickerOption) -> Option<String> {
    // Provider defaults of an SSH-remote session are not the local config's.
    if crate::tui::is_ssh_remote() {
        return None;
    }
    let selection = crate::provider::MultiProvider::default_model_selection_from_route(
        model,
        &route.api_method,
        &route.provider,
    );
    let config = crate::config::config();
    match selection.provider_key.as_deref() {
        Some("claude-oauth") | Some("claude-api") => {
            config.provider.anthropic_reasoning_effort.clone()
        }
        Some("openai-oauth") | Some("openai-api") => {
            config.provider.openai_reasoning_effort.clone()
        }
        _ => None,
    }
}

pub(super) fn model_effort_step_preselection(
    levels: &[&str],
    current_effort: Option<&str>,
    saved_effort: Option<&str>,
) -> usize {
    let find = |wanted: &str| levels.iter().position(|e| e.eq_ignore_ascii_case(wanted));
    current_effort
        .and_then(find)
        .or_else(|| saved_effort.and_then(find))
        .or_else(|| find("high"))
        .unwrap_or(0)
}

impl App {
    pub(super) fn apply_model_picker_choice(&mut self, entry: PickerEntry, effort: Option<String>) {
        let Some(route) = entry.options.get(entry.selected_option) else {
            return;
        };
        let effort_suffix = effort.as_deref().map_or_else(String::new, |effort| {
            format!(" ({})", reasoning_effort_picker_label(effort))
        });
        if !route.available {
            self.push_display_message(DisplayMessage::error(
                crate::tui::app::model_context::unavailable_model_route_message(
                    &entry.name,
                    &route.provider,
                    &route.detail,
                    self.is_remote,
                ),
            ));
            self.set_status_notice("Model unavailable");
            return;
        }

        let bare_name = model_entry_base_name(&entry);
        let spec = if crate::provider::ModelRouteApiMethod::parse(&route.api_method).is_openrouter()
            && route.provider == "auto"
        {
            openrouter_route_model_id(&bare_name)
        } else {
            picker_route_model_spec(&entry, route)
        };
        let route_selection = picker_route_selection(&entry, route);

        record_model_picker_selection(&bare_name, route);
        let method_label =
            crate::provider::ModelRouteApiMethod::parse(&route.api_method).display_label();
        // Placeholder routes ("remote-catalog"/"current") are
        // catalog-refresh stand-ins, not real provider routes:
        // surfacing them produced confusing notices like
        // "Model → x via Claude (remote-catalog) · refreshing
        // route details…" right before the real switch
        // confirmation. Show just the model in that case.
        let placeholder_route = placeholder_routes::is_placeholder_route_method(&route.api_method);
        let notice = if placeholder_route {
            format!("Model → {}{}", entry.name, effort_suffix)
        } else {
            format!(
                "Model → {} via {} ({})",
                entry.name, route.provider, method_label
            )
        };
        let route_detail = if placeholder_route {
            String::new()
        } else {
            route.detail.trim().to_string()
        };

        // Record exactly which model spec + route the user chose
        // and how it will be applied. Pairs with the server-side
        // model-switch logs so we can trace a `/model` choice all
        // the way to the provider endpoint that ends up serving it
        // (issues #292/#278: switch routes to wrong endpoint).
        crate::logging::event_info(
            "model_picker_select",
            vec![
                ("entry", entry.name.clone()),
                ("spec", spec.clone()),
                ("provider", route.provider.clone()),
                ("api_method", route.api_method.clone()),
                ("route_provider", route_selection.provider_label.clone()),
                ("route_model", route_selection.model.clone()),
                ("route_api_method", route_selection.api_method.clone()),
                (
                    "effort",
                    effort.clone().unwrap_or_else(|| "none".to_string()),
                ),
                ("remote", self.is_remote.to_string()),
            ],
        );

        if self.is_remote {
            self.inline_interactive_state = None;
            self.upstream_provider = None;
            self.status_detail = None;
            // Track the chosen method client-side so post-error
            // fallback picks know which credential path the
            // active route uses (remote sessions have no other
            // route bookkeeping).
            self.session.route_api_method = Some(route_selection.api_method.clone());
            self.pending_route_selection = Some(route_selection);
            self.pending_model_switch = Some(spec);
            // In remote mode `self.provider` is a local
            // stand-in, so applying the picked level to it does
            // not reach the server. Stage it so the remote
            // dispatcher forwards it right after the model
            // switch; otherwise the server keeps its configured
            // default (low) and silently runs e.g. gpt-5.5
            // picked at high at low effort (issue #427).
            self.pending_reasoning_effort = effort.clone();
        } else {
            match self.provider.set_route_selection(&route_selection) {
                Ok(()) => {
                    self.inline_interactive_state = None;
                    self.provider_session_id = None;
                    self.session.provider_session_id = None;
                    self.upstream_provider = None;
                    self.status_detail = None;
                    self.invalidate_model_picker_cache();
                    let active_model = self.provider.model();
                    self.update_context_limit_for_model(&active_model, None);
                    self.session.provider_key =
                        crate::provider::MultiProvider::session_provider_key_after_model_switch(
                            &spec,
                            self.provider.name(),
                            self.session.provider_key.as_deref(),
                        );
                    self.session.model = Some(active_model.clone());
                    self.session.route_api_method = Some(route_selection.api_method.clone());
                    if let Err(error) = self.session.save() {
                        crate::logging::error(&format!("Failed to save selected model: {error}"));
                    }
                    crate::logging::event_info(
                        "model_picker_select_applied",
                        vec![
                            ("spec", spec.clone()),
                            ("active_model", active_model),
                            ("provider", self.provider.name().to_string()),
                            ("api_method", route_selection.api_method.clone()),
                        ],
                    );
                }
                Err(error) => {
                    crate::logging::event_error(
                        "model_picker_select_failed",
                        vec![
                            ("spec", spec.clone()),
                            ("provider", route.provider.clone()),
                            ("api_method", route_selection.api_method.clone()),
                            ("error", error.to_string()),
                        ],
                    );
                    self.push_display_message(DisplayMessage::error(
                        crate::tui::app::model_context::model_switch_failure_message(
                            &error.to_string(),
                            self.is_remote,
                        ),
                    ));
                    self.set_status_notice("Model switch failed");
                    return;
                }
            }
        }
        if !self.is_remote
            && let Some(effort) = effort
            && let Err(error) = self.provider.set_reasoning_effort(&effort)
        {
            self.push_display_message(DisplayMessage::error(format!(
                "Model switched, but reasoning level was rejected: {error}"
            )));
            self.set_status_notice("Model switched; reasoning level rejected");
            return;
        }
        if !self.is_remote && !route_detail.is_empty() {
            self.push_display_message(DisplayMessage::system(format!(
                "{}\n{}",
                notice, route_detail
            )));
        }
        self.set_status_notice(if route_detail.is_empty() {
            notice
        } else {
            format!("{} · {}", notice, route_detail)
        });
        // First-run onboarding: a model choice advances the flow.
        self.onboarding_after_model_select();
    }

    pub(super) fn model_picker_save_default_key(&mut self) {
        if crate::tui::app::commands_dispatch::ssh_local_action_blocked(
            self,
            "Saving a default model",
        ) {
            return;
        }
        let Some(picker) = self.inline_interactive_state.as_ref() else {
            return;
        };
        if !picker_is_runtime_model_picker(picker) {
            return;
        }
        let Some(idx) = picker.selected_entry_index() else {
            return;
        };
        let entry = &picker.entries[idx];
        if !matches!(entry.action, PickerAction::Model) {
            return;
        }
        let has_levels = entry
            .active_option()
            .is_some_and(|route| !model_route_effort_levels(&entry.name, route).is_empty());
        if has_levels {
            self.open_model_effort_step(true);
        } else {
            let entry = entry.clone();
            self.save_model_picker_default(idx, &entry, None);
        }
    }

    pub(super) fn save_model_picker_default(
        &mut self,
        idx: usize,
        entry: &PickerEntry,
        effort: Option<&str>,
    ) {
        let bare_name = model_entry_base_name(entry);
        let (model_spec, provider_key) = if let Some(r) = entry.active_option() {
            let selection = crate::provider::MultiProvider::default_model_selection_from_route(
                &bare_name,
                &r.api_method,
                &r.provider,
            );
            (selection.model_spec, selection.provider_key)
        } else {
            (bare_name.clone(), None)
        };
        let effort_suffix = effort.map_or_else(String::new, |effort| {
            format!(" ({})", reasoning_effort_picker_label(effort))
        });

        let notice = format!(
            "Default → {}{} via {}",
            model_spec,
            effort_suffix,
            provider_key.as_deref().unwrap_or("auto")
        );

        match crate::config::Config::set_default_model(Some(&model_spec), provider_key.as_deref()) {
            Ok(()) => {
                // Persist the level picked in the reasoning step so the
                // default survives restarts with its effort (issue #675).
                if let Some(effort) = effort {
                    let save_result = match provider_key.as_deref() {
                        Some("claude-oauth") | Some("claude-api") => Some(
                            crate::config::Config::set_anthropic_reasoning_effort(Some(effort)),
                        ),
                        Some("openai-oauth") | Some("openai-api") => Some(
                            crate::config::Config::set_openai_reasoning_effort(Some(effort)),
                        ),
                        _ => None,
                    };
                    if let Some(Err(e)) = save_result {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Saved default model, but failed to save effort: {}",
                            e
                        )));
                    }
                }
                self.invalidate_model_picker_cache();
                if let Some(ref mut picker) = self.inline_interactive_state {
                    for entry in &mut picker.entries {
                        entry.is_default = false;
                    }
                    if let Some(entry) = picker.entries.get_mut(idx) {
                        entry.is_default = true;
                    }
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Saved default model: {}{} via {}. This affects future sessions.",
                    model_spec,
                    effort_suffix,
                    provider_key.as_deref().unwrap_or("auto")
                )));
                self.set_status_notice(notice)
            }
            Err(e) => self.set_status_notice(format!("Failed to save default: {}", e)),
        }
    }

    pub(super) fn open_model_effort_step(&mut self, save_default: bool) {
        let current_model = if self.is_remote {
            self.remote_provider_model
                .as_deref()
                .unwrap_or("")
                .to_string()
        } else {
            self.provider.model()
        };
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let Some(mut parent) = self.inline_interactive_state.take() else {
            return;
        };
        let Some(model) = parent.selected_entry().cloned() else {
            self.inline_interactive_state = Some(parent);
            return;
        };
        let Some(route) = model.active_option().cloned() else {
            self.inline_interactive_state = Some(parent);
            return;
        };
        let levels = model_route_effort_levels(&model.name, &route);
        // The session's level belongs to the route it runs on. Another route
        // of the same model is a different choice and starts on the saved
        // level. Match by name only when no row knows the current route
        // (for example a names-only remote catalog).
        let any_row_is_current = parent.entries.iter().any(|entry| entry.is_current);
        let is_current_model =
            model.is_current || (!any_row_is_current && model.name == current_model);
        let current_effort = current_effort.filter(|_| is_current_model);
        let saved_effort = saved_default_reasoning_effort(&model.name, &route);
        let selected = model_effort_step_preselection(
            &levels,
            current_effort.as_deref(),
            saved_effort.as_deref(),
        );

        let entries: Vec<PickerEntry> = levels
            .iter()
            .map(|effort| PickerEntry {
                name: reasoning_effort_picker_label(effort).to_string(),
                options: Vec::new(),
                action: PickerAction::Model,
                selected_option: 0,
                is_current: current_effort.as_deref() == Some(*effort),
                is_default: false,
                is_favorite: false,
                recommended: *effort == "high",
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: Some((*effort).to_string()),
            })
            .collect();

        // The typed `/model …` text belonged to the list's preview. The step
        // is focused, and Esc goes back to a focused list with that filter.
        if parent.preview {
            parent.preview = false;
            self.input.clear();
            self.cursor_pos = 0;
        }
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            selected,
            column: 0,
            filter: String::new(),
            preview: false,
            scoped_route_restore: Vec::new(),
            effort_step: Some(Box::new(crate::tui::ModelEffortStep {
                model,
                parent,
                save_default,
                parent_needs_refresh: false,
            })),
        });
    }

    pub(super) fn handle_model_effort_step_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(picker) = self.inline_interactive_state.as_mut() else {
            return;
        };
        let last = picker.filtered.len().saturating_sub(1);
        let mut refresh_parent = false;
        match code {
            KeyCode::Esc => {
                if let Some(step) = picker.effort_step.take() {
                    refresh_parent = step.parent_needs_refresh;
                    self.inline_interactive_state = Some(step.parent);
                }
            }
            KeyCode::Up | KeyCode::BackTab => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                picker.selected = picker.selected.saturating_sub(1)
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                picker.selected = picker.selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') | KeyCode::Char('J') => {
                picker.selected = (picker.selected + 1).min(last)
            }
            KeyCode::Home | KeyCode::PageUp => picker.selected = 0,
            KeyCode::End | KeyCode::PageDown => picker.selected = last,
            KeyCode::Enter => {
                let effort = picker
                    .selected_entry()
                    .and_then(|entry| entry.effort.clone());
                let Some(step) = picker.effort_step.take() else {
                    return;
                };
                let crate::tui::ModelEffortStep {
                    model,
                    parent,
                    save_default,
                    parent_needs_refresh,
                } = *step;
                refresh_parent = parent_needs_refresh;
                let parent_index = parent.selected_entry_index();
                // Back to the list first: a failed switch or a saved default
                // leaves the user in the model list, not in the step.
                self.inline_interactive_state = Some(parent);
                if save_default {
                    if let Some(idx) = parent_index {
                        self.save_model_picker_default(idx, &model, effort.as_deref());
                    }
                } else {
                    self.apply_model_picker_choice(model, effort);
                }
            }
            _ => {}
        }

        // Catalogs may change while the level chooser holds the model list.
        if refresh_parent
            && let Some(previous) = self
                .inline_interactive_state
                .as_ref()
                .filter(|picker| picker_is_runtime_model_picker(picker))
                .cloned()
        {
            self.refresh_open_model_picker_after_catalog_update();
            if let Some(picker) = self.inline_interactive_state.as_mut() {
                picker.preview = previous.preview;
                picker.filter = previous.filter.clone();
                restore_model_browser_after_refresh(picker, &previous);
                picker.column = previous.column.min(picker.max_navigable_column());
            }
        }
    }
}
