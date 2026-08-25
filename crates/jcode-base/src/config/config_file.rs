use super::*;
use crate::storage::jcode_dir;
use std::path::PathBuf;

impl Config {
    /// Get the config file path
    pub fn path() -> Option<PathBuf> {
        jcode_dir().ok().map(|d| d.join("config.toml"))
    }

    /// Load config from file, with environment variable overrides
    pub fn load() -> Self {
        let mut config = Self::load_from_file().unwrap_or_default();
        config.apply_env_overrides();
        config
    }

    /// Load config from file, with environment variable overrides.
    ///
    /// Unlike [`Self::load`], this returns TOML/read errors to callers that need
    /// to distinguish a malformed config from an absent config.
    pub fn load_strict() -> anyhow::Result<Self> {
        let mut config = Self::load_from_file_strict()?.unwrap_or_default();
        config.apply_env_overrides();
        Ok(config)
    }

    /// Load the on-disk config for a read-modify-write operation.
    ///
    /// Unlike [`Self::load`], this never converts a parse error into defaults.
    /// Saving those defaults would destroy the user's existing config. It also
    /// deliberately skips environment overrides so transient process settings
    /// are not baked into the file as a side effect of changing one preference.
    fn load_for_update() -> anyhow::Result<Self> {
        Ok(Self::load_from_file_strict()?.unwrap_or_default())
    }

    /// Load config from file only (no env overrides)
    fn load_from_file() -> Option<Self> {
        match Self::load_from_file_strict() {
            Ok(config) => config,
            Err(e) => {
                crate::logging::error(&format!("Failed to parse config file: {}", e));
                None
            }
        }
    }

    /// Load config from file only (no env overrides), preserving parse/read errors.
    fn load_from_file_strict() -> anyhow::Result<Option<Self>> {
        let Some(path) = Self::path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file {}: {}", path.display(), e))?;
        let mut config = toml::from_str::<Self>(&content).map_err(|e| {
            anyhow::anyhow!("Failed to parse config file {}: {}", path.display(), e)
        })?;
        config.display.apply_legacy_compat();
        config.repair_frozen_sponsors_optout(&content);
        Ok(Some(config))
    }

    /// Undo a machine-frozen partner-discovery opt-out.
    ///
    /// Discovery shipped opt-in (`enabled = false`), and because [`Self::save`]
    /// serializes the whole struct, any config write during that window baked
    /// the old default into the user's file. Those users keep discovery
    /// permanently disabled even after the default flipped to opt-out, and
    /// telemetry shows this is the single largest discovery blocker.
    ///
    /// A machine-written section is exactly `enabled` plus `endpoint` with a
    /// known default endpoint. A hand-written opt-out (`enabled = false` alone,
    /// or paired with a custom endpoint) is always respected. Repair happens in
    /// memory only; the section then disappears on the next save because it
    /// serializes back to the default.
    pub(crate) fn repair_frozen_sponsors_optout(&mut self, raw: &str) {
        if self.sponsors.enabled {
            return;
        }
        let Ok(doc) = raw.parse::<toml::Value>() else {
            return;
        };
        let Some(table) = doc.get("sponsors").and_then(toml::Value::as_table) else {
            return;
        };
        let machine_written = table.len() == 2
            && table.get("enabled").and_then(toml::Value::as_bool) == Some(false)
            && table
                .get("endpoint")
                .and_then(toml::Value::as_str)
                .is_some_and(super::is_default_discovery_endpoint);
        if !machine_written {
            return;
        }
        self.sponsors = SponsorsConfig::default();
        crate::logging::info(
            "config: restored integration discovery default (legacy opt-in value was frozen by an \
             earlier config save)",
        );
    }

    /// Save config to file
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("No config path"))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Self::invalidate_cache();
        Ok(())
    }

    /// Mark the process-cached config as stale and notify dependent caches.
    pub fn invalidate_cache() {
        super::invalidate_config_cache();
    }

    /// Update the copilot premium mode in the config file.
    /// Reloads, patches, and saves so it doesn't clobber other fields.
    pub fn set_copilot_premium(mode: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.copilot_premium = mode.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved copilot_premium to config: {}",
            mode.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update just the default model and provider in the config file.
    /// This reloads, patches, and saves so it doesn't clobber other fields.
    pub fn set_default_model(model: Option<&str>, provider: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.default_model = model.map(|s| s.to_string());
        cfg.provider.default_provider = provider.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved default model: {}, provider: {}",
            model.unwrap_or("(none)"),
            provider.unwrap_or("(auto)")
        ));
        Ok(())
    }

    /// Update just the default provider in the config file.
    pub fn set_default_provider(provider: Option<&str>) -> anyhow::Result<()> {
        let cfg = Self::load_for_update()?;
        Self::set_default_model(cfg.provider.default_model.as_deref(), provider)
    }

    /// Update just the default model in the config file.
    pub fn set_default_model_only(model: Option<&str>) -> anyhow::Result<()> {
        let cfg = Self::load_for_update()?;
        Self::set_default_model(model, cfg.provider.default_provider.as_deref())
    }

    /// Update the persisted OpenAI reasoning effort preference.
    pub fn set_openai_reasoning_effort(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.openai_reasoning_effort = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_reasoning_effort to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted Anthropic reasoning effort preference.
    pub fn set_anthropic_reasoning_effort(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.anthropic_reasoning_effort = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved anthropic_reasoning_effort to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted OpenAI transport preference.
    pub fn set_openai_transport(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.openai_transport = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_transport to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted OpenAI service tier preference.
    pub fn set_openai_service_tier(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.provider.openai_service_tier = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_service_tier to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted default alignment preference.
    pub fn set_display_centered(centered: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.centered = centered;
        cfg.save()?;
        crate::logging::info(&format!("Saved display.centered to config: {}", centered));
        Ok(())
    }

    /// Update the persisted reasoning display mode preference.
    pub fn set_reasoning_display(mode: ReasoningDisplayMode) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.set_reasoning_display(mode);
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved display.reasoning_display to config: {}",
            mode.label()
        ));
        Ok(())
    }

    /// Update the persisted compact-notifications preference.
    pub fn set_compact_notifications(compact: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.compact_notifications = compact;
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved display.compact_notifications to config: {}",
            compact
        ));
        Ok(())
    }

    /// Update the persisted pinned-todos preference.
    pub fn set_pin_todos(pin: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.pin_todos = pin;
        cfg.save()?;
        crate::logging::info(&format!("Saved display.pin_todos to config: {}", pin));
        Ok(())
    }

    /// Update the persisted show-agentgrep-output preference.
    pub fn set_show_agentgrep_output(show: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.show_agentgrep_output = show;
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved display.show_agentgrep_output to config: {}",
            show
        ));
        Ok(())
    }

    /// Update the persisted tool-call-details preference.
    pub fn set_tool_call_details(show: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.display.tool_call_details = show;
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved display.tool_call_details to config: {}",
            show
        ));
        Ok(())
    }

    /// Persist the baked global launch-hotkey mapping.
    ///
    /// Auto-import calls this once with the per-repo chord -> directory layout it
    /// inferred. `imported` is set so the bake never runs twice and later manual
    /// edits are not clobbered.
    pub fn set_launch_hotkeys(
        entries: Vec<jcode_config_types::LaunchHotkeyEntry>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let mut cfg = Self::load_for_update()?;
        cfg.launch_hotkeys.entries = entries;
        cfg.launch_hotkeys.enabled = Some(enabled);
        cfg.launch_hotkeys.imported = true;
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved {} launch hotkey(s) to config (enabled={enabled})",
            cfg.launch_hotkeys.entries.len()
        ));
        Ok(())
    }

    /// One-time bake of per-repo launch hotkeys from session history.
    ///
    /// Scans `~/.jcode/sessions` for the directories the user works in most,
    /// ranks them (recency-weighted, git-root folded, home excluded), and writes
    /// a static chord -> directory mapping into config: top repo on `Cmd+;`, home
    /// on `Cmd+'`, and the next repos on `Cmd+[` / `Cmd+]` / `Cmd+\`.
    ///
    /// Idempotent and side-effect-light:
    /// - Runs only on platforms with global launch hotkeys (macOS, Linux,
    ///   Windows).
    /// - No-ops once `launch_hotkeys.imported` is set, so it bakes exactly once
    ///   and never overwrites later manual edits.
    /// - No-ops when there are not at least two rankable repos, so we do not
    ///   commit a degenerate "everything is home" layout on a fresh machine; the
    ///   built-in 3 hotkeys keep working until there is real history.
    ///
    /// Returns `true` when it wrote a baked mapping (so the caller can trigger a
    /// hotkey reinstall), `false` otherwise. Best-effort: errors are logged and
    /// swallowed.
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    pub fn bake_launch_hotkeys_once() -> bool {
        use jcode_import_core::repo_ranking;

        let cfg = Self::load();
        if cfg.launch_hotkeys.imported {
            return false;
        }
        let Ok(jcode_dir) = jcode_dir() else {
            return false;
        };
        let sessions_dir = jcode_dir.join("sessions");
        let Some(home) = dirs::home_dir() else {
            return false;
        };

        // Cheap gate: count session files without reading them. Skip the full
        // scan until there is at least a little history, so brand-new installs do
        // not pay the read cost (and we do not bake a degenerate layout).
        let session_count = std::fs::read_dir(&sessions_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".json")))
                    .count()
            })
            .unwrap_or(0);
        const MIN_SESSIONS_TO_BAKE: usize = 3;
        const GIVE_UP_SESSION_COUNT: usize = 50;
        if session_count < MIN_SESSIONS_TO_BAKE {
            return false;
        }

        let plan = repo_ranking::plan_launch_hotkeys_from_sessions(
            &sessions_dir,
            &home,
            chrono::Utc::now(),
        );

        // `plan` always contains the home slot; a length of 1 means no rankable
        // repos were found.
        if plan.len() < 2 {
            // If the user has lots of history but still no rankable repos, stop
            // re-scanning on every launch: mark imported with no custom entries
            // (the built-in 3 hotkeys keep working).
            if session_count >= GIVE_UP_SESSION_COUNT
                && let Err(err) = Self::set_launch_hotkeys(Vec::new(), true)
            {
                crate::logging::warn(&format!("launch hotkey bake give-up persist failed: {err}"));
            }
            crate::logging::info(
                "launch hotkey bake: not enough repo history yet; keeping defaults",
            );
            return false;
        }

        let entries: Vec<jcode_config_types::LaunchHotkeyEntry> = plan
            .into_iter()
            .map(|p| jcode_config_types::LaunchHotkeyEntry {
                chord: p.chord,
                // Home keeps the dynamic sentinel so it tracks `$HOME`; repos are
                // baked to absolute paths.
                dir: if p.label == "home" {
                    "$HOME".to_string()
                } else {
                    p.dir
                },
                label: p.label,
                self_dev: false,
            })
            .collect();

        match Self::set_launch_hotkeys(entries, true) {
            Ok(()) => {
                crate::logging::info("launch hotkey bake: wrote per-repo mapping to config");
                true
            }
            Err(err) => {
                crate::logging::warn(&format!("launch hotkey bake failed to persist: {err}"));
                false
            }
        }
    }

    /// No-op bake on platforms without global launch hotkeys.
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    pub fn bake_launch_hotkeys_once() -> bool {
        false
    }

    /// One-time migration: flip a persisted legacy `swarm_spawn_mode =
    /// "visible"` to the current `"inline"` default.
    ///
    /// Historically `visible` was the default, and any full-config
    /// `Config::save()` (model switches, display toggles, ...) baked that
    /// then-default into the user's config.toml. When the default changed to
    /// `inline`, those users stayed pinned to `visible` forever. This rewrites
    /// exactly that one line (preserving the rest of the file byte-for-byte)
    /// and drops a marker so it runs at most once. A user who explicitly sets
    /// `visible` after the migration is never flipped again.
    ///
    /// Returns `true` when it rewrote the config. Best-effort: errors are
    /// logged and swallowed.
    pub fn migrate_legacy_swarm_spawn_mode_once() -> bool {
        let Ok(dir) = jcode_dir() else {
            return false;
        };
        let marker = dir.join("migrations").join("swarm-spawn-mode-inline");
        if marker.exists() {
            return false;
        }
        let write_marker = || {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(
                &marker,
                "swarm_spawn_mode default migration: visible -> inline\n",
            );
        };

        let path = dir.join("config.toml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            // No config file (fresh install): nothing to migrate.
            write_marker();
            return false;
        };

        let mut changed = false;
        let migrated: Vec<String> = content
            .lines()
            .map(|line| {
                if changed {
                    return line.to_string();
                }
                let trimmed = line.trim_start();
                let Some(rest) = trimmed.strip_prefix("swarm_spawn_mode") else {
                    return line.to_string();
                };
                let Some(value) = rest.trim_start().strip_prefix('=') else {
                    return line.to_string();
                };
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                if matches!(value, "visible" | "headed") {
                    changed = true;
                    let indent = &line[..line.len() - trimmed.len()];
                    format!("{indent}swarm_spawn_mode = \"inline\"")
                } else {
                    line.to_string()
                }
            })
            .collect();

        if !changed {
            write_marker();
            return false;
        }

        let mut new_content = migrated.join("\n");
        if content.ends_with('\n') {
            new_content.push('\n');
        }
        match std::fs::write(&path, new_content) {
            Ok(()) => {
                Self::invalidate_cache();
                write_marker();
                crate::logging::info(
                    "Migrated legacy swarm_spawn_mode \"visible\" to \"inline\" in config.toml",
                );
                true
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "swarm_spawn_mode migration failed to write config: {err}"
                ));
                false
            }
        }
    }

    /// One-time migration: flip a persisted `idle_animation = true` to `false`.
    ///
    /// The idle animation is being turned off for everyone. Users who toggled
    /// it on earlier (or had the old `true` default baked in by a full
    /// `Config::save()`) get flipped off once. This rewrites exactly that one
    /// line (preserving the rest of the file byte-for-byte) and drops a marker
    /// so it runs at most once. A user who explicitly re-enables it after the
    /// migration is never flipped again.
    ///
    /// Returns `true` when it rewrote the config. Best-effort: errors are
    /// logged and swallowed.
    pub fn migrate_idle_animation_off_once() -> bool {
        let Ok(dir) = jcode_dir() else {
            return false;
        };
        let marker = dir.join("migrations").join("idle-animation-off");
        if marker.exists() {
            return false;
        }
        let write_marker = || {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker, "idle_animation forced migration: true -> false\n");
        };

        let path = dir.join("config.toml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            // No config file (fresh install): nothing to migrate.
            write_marker();
            return false;
        };

        let mut changed = false;
        let migrated: Vec<String> = content
            .lines()
            .map(|line| {
                if changed {
                    return line.to_string();
                }
                let trimmed = line.trim_start();
                let Some(rest) = trimmed.strip_prefix("idle_animation") else {
                    return line.to_string();
                };
                let Some(value) = rest.trim_start().strip_prefix('=') else {
                    return line.to_string();
                };
                let value = value.split('#').next().unwrap_or("");
                if value.trim() == "true" {
                    changed = true;
                    let indent = &line[..line.len() - trimmed.len()];
                    format!("{indent}idle_animation = false")
                } else {
                    line.to_string()
                }
            })
            .collect();

        if !changed {
            write_marker();
            return false;
        }

        let mut new_content = migrated.join("\n");
        if content.ends_with('\n') {
            new_content.push('\n');
        }
        match std::fs::write(&path, new_content) {
            Ok(()) => {
                Self::invalidate_cache();
                write_marker();
                crate::logging::info(
                    "Migrated idle_animation \"true\" to \"false\" in config.toml",
                );
                true
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "idle_animation migration failed to write config: {err}"
                ));
                false
            }
        }
    }

    fn normalize_external_auth_source_id(source_id: &str) -> String {
        source_id.trim().to_ascii_lowercase()
    }

    pub(crate) fn trusted_external_auth_path_entry(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<String> {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            anyhow::bail!("External auth source id cannot be empty");
        }
        let canonical = crate::storage::validate_external_auth_file(path)?;
        Ok(format!(
            "{}|{}",
            source_id,
            canonical.to_string_lossy().to_ascii_lowercase()
        ))
    }

    pub fn external_auth_source_allowed(source_id: &str) -> bool {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            return false;
        }

        let cfg = Self::load();
        cfg.auth
            .trusted_external_sources
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&source_id))
    }

    pub fn external_auth_source_allowed_for_path(source_id: &str, path: &std::path::Path) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };

        let cfg = Self::load();
        cfg.auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }

    /// Startup-sensitive variant that uses the process-cached config snapshot.
    ///
    /// This avoids reloading config.toml repeatedly during cold-start probes.
    pub fn external_auth_source_allowed_for_path_cached(
        source_id: &str,
        path: &std::path::Path,
    ) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };

        if config()
            .auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
        {
            return true;
        }

        // The global config snapshot can be initialized before an auth flow saves
        // a new path-bound trust decision, or before tests switch JCODE_HOME. Fall
        // back to a fresh load on cache misses so fast auth probes remain correct
        // without penalizing the common already-trusted path.
        Self::load()
            .auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }

    pub fn allow_external_auth_source(source_id: &str) -> anyhow::Result<()> {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            anyhow::bail!("External auth source id cannot be empty");
        }

        let mut cfg = Self::load_for_update()?;
        if !cfg
            .auth
            .trusted_external_sources
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&source_id))
        {
            cfg.auth.trusted_external_sources.push(source_id.clone());
            cfg.auth.trusted_external_sources.sort();
            cfg.auth.trusted_external_sources.dedup();
            cfg.save()?;
        }

        crate::logging::info(&format!(
            "Saved trusted external auth source to config: {}",
            source_id
        ));
        Ok(())
    }

    pub fn allow_external_auth_source_for_path(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let entry = Self::trusted_external_auth_path_entry(source_id, path)?;
        let mut cfg = Self::load_for_update()?;
        if !cfg
            .auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
        {
            cfg.auth.trusted_external_source_paths.push(entry.clone());
            cfg.auth.trusted_external_source_paths.sort();
            cfg.auth.trusted_external_source_paths.dedup();
            cfg.save()?;
        }
        crate::logging::info(&format!(
            "Saved trusted external auth source path: {}",
            entry
        ));
        Ok(())
    }

    pub fn revoke_external_auth_source_for_path(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let entry = Self::trusted_external_auth_path_entry(source_id, path)?;
        let mut cfg = Self::load_for_update()?;
        let before = cfg.auth.trusted_external_source_paths.len();
        cfg.auth
            .trusted_external_source_paths
            .retain(|value| !value.trim().eq_ignore_ascii_case(&entry));
        if cfg.auth.trusted_external_source_paths.len() != before {
            cfg.save()?;
            crate::logging::info(&format!(
                "Removed trusted external auth source path: {}",
                entry
            ));
        }
        Ok(())
    }

    /// Remove a source-level (non-path) trust decision, e.g. for credentials
    /// that have no stable on-disk path (macOS Keychain items).
    pub fn revoke_external_auth_source(source_id: &str) -> anyhow::Result<()> {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            return Ok(());
        }
        let mut cfg = Self::load_for_update()?;
        let before = cfg.auth.trusted_external_sources.len();
        cfg.auth
            .trusted_external_sources
            .retain(|value| !value.trim().eq_ignore_ascii_case(&source_id));
        if cfg.auth.trusted_external_sources.len() != before {
            cfg.save()?;
            crate::logging::info(&format!(
                "Removed trusted external auth source: {}",
                source_id
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod issue_1056_tests {
    use super::Config;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => crate::env::set_var(self.key, value),
                None => crate::env::remove_var(self.key),
            }
            Config::invalidate_cache();
        }
    }

    #[test]
    fn effort_update_preserves_profile_with_capitalized_bearer_auth() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let path = home.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[provider]
openai_reasoning_effort = "low"

[providers.mistral]
type = "openai-compatible"
base_url = "https://api.mistral.ai/v1"
auth = "Bearer"
api_key_env = "MISTRAL_API_KEY"
disable_reasoning_heuristics = true

[[providers.mistral.models]]
id = "mistral-medium-latest"
reasoning = true
reasoning_effort = "max"
"#,
        )
        .unwrap();

        Config::set_openai_reasoning_effort(Some("high")).unwrap();

        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("[providers.mistral]"));
        assert!(saved.contains("mistral-medium-latest"));
        let parsed = Config::load_strict().unwrap();
        assert_eq!(
            parsed.provider.openai_reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(parsed.providers["mistral"].models.len(), 1);
    }

    #[test]
    fn effort_update_refuses_to_overwrite_malformed_config() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let path = home.path().join("config.toml");
        let original = "[providers.broken]\nauth = \"invalid-auth-mode\"\n";
        std::fs::write(&path, original).unwrap();

        let error = Config::set_openai_reasoning_effort(Some("high"))
            .expect_err("a malformed config must block mutation");

        assert!(error.to_string().contains("Failed to parse config file"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}
