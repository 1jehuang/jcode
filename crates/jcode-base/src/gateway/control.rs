//! Shared control logic behind the `/remote` command and `jcode pair`.
//!
//! The WebSocket gateway already speaks the full session protocol, so making a
//! session reachable from another machine is only ever three questions: is the
//! gateway enabled, what address should the other machine dial, and which
//! devices are allowed in. This module answers those in one place so the TUI
//! and the CLI cannot drift apart.

use anyhow::Result;

use super::{DeviceRegistry, resolve_connect_host};

/// Parsed `/remote` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    /// Activate or manage the subscription-backed Jcode Cloud host.
    Cloud,
    /// Show gateway state, dial address, and paired devices.
    Status,
    /// Enable the gateway in config.
    On,
    /// Disable the gateway in config.
    Off,
    /// Mint a pairing code for a new device.
    Pair,
    /// Remove a paired device by id or name.
    Revoke(String),
    /// Usage text.
    Help,
}

/// Parse a `/remote` command line.
///
/// Returns `None` when the input is not `/remote` at all, so the caller can
/// fall through to other commands. Returns `Err` for a malformed `/remote`.
pub fn parse_remote_command(input: &str) -> Option<Result<RemoteCommand, String>> {
    let trimmed = input.trim();
    let rest = trimmed
        .strip_prefix("/remote")
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))?;

    let mut parts = rest.split_whitespace();
    let Some(sub) = parts.next() else {
        return Some(Ok(RemoteCommand::Cloud));
    };

    let command = match sub.to_ascii_lowercase().as_str() {
        "cloud" | "setup" => RemoteCommand::Cloud,
        "status" | "local" => RemoteCommand::Status,
        "on" | "enable" => RemoteCommand::On,
        "off" | "disable" => RemoteCommand::Off,
        "pair" => RemoteCommand::Pair,
        "help" => RemoteCommand::Help,
        "revoke" | "unpair" => {
            let target = parts.clone().collect::<Vec<_>>().join(" ");
            if target.trim().is_empty() {
                return Some(Err("Usage: /remote revoke <device-id-or-name>".to_string()));
            }
            RemoteCommand::Revoke(target.trim().to_string())
        }
        other => {
            return Some(Err(format!(
                "Unknown /remote subcommand: {other}\nUsage: /remote [cloud|status|on|off|pair|revoke <device>]"
            )));
        }
    };

    // Only `revoke` takes an argument.
    if !matches!(command, RemoteCommand::Revoke(_)) && parts.next().is_some() {
        return Some(Err(format!(
            "/remote {sub} takes no arguments\nUsage: /remote [cloud|status|on|off|pair|revoke <device>]"
        )));
    }

    Some(Ok(command))
}

/// A device that has been paired with this gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub paired_at: String,
    pub last_seen: String,
}

/// Everything `/remote status` needs to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub port: u16,
    pub bind_addr: String,
    /// Address a remote client should dial.
    pub connect_host: String,
    /// True when we could not work out a reachable name for this machine.
    pub host_unknown: bool,
    pub devices: Vec<DeviceSummary>,
}

impl RemoteStatus {
    /// Gather current gateway state.
    pub fn load() -> Self {
        let config = crate::config::config().gateway.clone();
        Self::from_parts(
            config.enabled,
            config.port,
            &config.bind_addr,
            DeviceRegistry::load(),
        )
    }

    fn from_parts(enabled: bool, port: u16, bind_addr: &str, registry: DeviceRegistry) -> Self {
        let connect_host = resolve_connect_host(bind_addr);
        Self {
            enabled,
            port,
            bind_addr: bind_addr.to_string(),
            host_unknown: connect_host == super::UNKNOWN_CONNECT_HOST,
            connect_host,
            devices: registry
                .devices
                .into_iter()
                .map(|d| DeviceSummary {
                    id: d.id,
                    name: d.name,
                    paired_at: d.paired_at,
                    last_seen: d.last_seen,
                })
                .collect(),
        }
    }

    /// `host:port` a remote client should dial.
    pub fn dial_address(&self) -> String {
        format!("{}:{}", self.connect_host, self.port)
    }

    /// Render status as markdown for the TUI transcript.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("**Remote access**\n\n");

        if self.enabled {
            out.push_str(&format!(
                "- Gateway: **on**, listening on `{}:{}`\n",
                self.bind_addr, self.port
            ));
            out.push_str(&format!(
                "- Dial from another machine: `{}`\n",
                self.dial_address()
            ));
        } else {
            out.push_str("- Gateway: **off**\n");
        }

        if self.devices.is_empty() {
            out.push_str("- Paired devices: none\n");
        } else {
            out.push_str(&format!("- Paired devices: {}\n", self.devices.len()));
            for device in &self.devices {
                out.push_str(&format!(
                    "  - `{}` ({}), last seen {}\n",
                    device.name, device.id, device.last_seen
                ));
            }
        }

        out.push('\n');
        if !self.enabled {
            out.push_str("Run `/remote on` to enable, then `/remote pair` to add a device.\n");
        } else if self.devices.is_empty() {
            out.push_str("Run `/remote pair` to authorize a device.\n");
        }

        if self.enabled && self.host_unknown {
            out.push_str(
                "\nCould not determine a reachable address for this machine. \
                 Set `JCODE_GATEWAY_HOST` to a hostname or IP the other machine can reach.\n",
            );
        }

        if self.enabled && is_wildcard_bind(&self.bind_addr) {
            out.push_str(
                "\nThe gateway accepts connections from any address that can reach this port. \
                 Keep it on a trusted network such as Tailscale rather than the public internet.\n",
            );
        }

        out
    }
}

/// Whether a bind address accepts connections on every interface.
pub fn is_wildcard_bind(bind_addr: &str) -> bool {
    bind_addr == "0.0.0.0" || bind_addr == "::"
}

/// Outcome of toggling the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Config changed; the server must restart to pick it up.
    Changed { enabled: bool },
    /// Config already had the requested value.
    Unchanged { enabled: bool },
}

/// Enable or disable the gateway, persisting to the config file.
///
/// Returns whether anything actually changed, because the gateway listener is
/// bound at server startup and a no-op change should not tell the user to
/// restart for nothing.
pub fn set_gateway_enabled(enabled: bool) -> Result<ToggleOutcome> {
    let mut config = crate::config::Config::load();
    if config.gateway.enabled == enabled {
        return Ok(ToggleOutcome::Unchanged { enabled });
    }
    config.gateway.enabled = enabled;
    config.save()?;
    crate::logging::info(&format!(
        "gateway: {} via /remote",
        if enabled { "enabled" } else { "disabled" }
    ));
    Ok(ToggleOutcome::Changed { enabled })
}

/// Remove a paired device by id or exact name.
///
/// Returns the removed device names, or an empty vec when nothing matched.
pub fn revoke_device(target: &str) -> Result<Vec<String>> {
    let mut registry = DeviceRegistry::load();
    let mut removed = Vec::new();
    registry.devices.retain(|device| {
        let matches = device.id == target || device.name == target;
        if matches {
            removed.push(device.name.clone());
        }
        !matches
    });
    if !removed.is_empty() {
        registry.save()?;
        crate::logging::info(&format!(
            "gateway: revoked device(s) {}",
            removed.join(", ")
        ));
    }
    Ok(removed)
}

/// A freshly minted pairing code and how a client should use it.
#[derive(Debug, Clone)]
pub struct PairingInvite {
    pub code: String,
    pub dial_address: String,
    pub uri: String,
    pub gateway_enabled: bool,
}

impl PairingInvite {
    /// Code formatted as `123 456` for reading aloud.
    pub fn spaced_code(&self) -> String {
        if self.code.len() == 6 {
            format!("{} {}", &self.code[..3], &self.code[3..])
        } else {
            self.code.clone()
        }
    }
}

/// Mint a pairing code valid for five minutes.
pub fn create_pairing_invite() -> Result<PairingInvite> {
    let status = RemoteStatus::load();
    let mut registry = DeviceRegistry::load();
    let code = registry.generate_pairing_code();
    registry.save()?;

    Ok(PairingInvite {
        uri: format!(
            "jcode://pair?host={}&port={}&code={}",
            status.connect_host, status.port, code
        ),
        dial_address: status.dial_address(),
        gateway_enabled: status.enabled,
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_gateway_types::PairedDevice;

    fn device(id: &str, name: &str) -> PairedDevice {
        PairedDevice {
            id: id.to_string(),
            name: name.to_string(),
            token_hash: "hash".to_string(),
            apns_token: None,
            paired_at: "2026-07-27".to_string(),
            last_seen: "2026-07-27".to_string(),
        }
    }

    fn registry(devices: Vec<PairedDevice>) -> DeviceRegistry {
        DeviceRegistry {
            devices,
            pending_codes: Vec::new(),
        }
    }

    #[test]
    fn bare_remote_starts_cloud_activation() {
        assert_eq!(
            parse_remote_command("/remote"),
            Some(Ok(RemoteCommand::Cloud))
        );
        assert_eq!(
            parse_remote_command("  /remote  "),
            Some(Ok(RemoteCommand::Cloud))
        );
    }

    #[test]
    fn subcommands_and_aliases_parse() {
        for (input, expected) in [
            ("/remote status", RemoteCommand::Status),
            ("/remote cloud", RemoteCommand::Cloud),
            ("/remote setup", RemoteCommand::Cloud),
            ("/remote local", RemoteCommand::Status),
            ("/remote on", RemoteCommand::On),
            ("/remote enable", RemoteCommand::On),
            ("/remote off", RemoteCommand::Off),
            ("/remote disable", RemoteCommand::Off),
            ("/remote pair", RemoteCommand::Pair),
            ("/remote help", RemoteCommand::Help),
            ("/remote ON", RemoteCommand::On),
        ] {
            assert_eq!(parse_remote_command(input), Some(Ok(expected)), "{input}");
        }
    }

    #[test]
    fn revoke_captures_target_including_spaced_names() {
        assert_eq!(
            parse_remote_command("/remote revoke phone-1"),
            Some(Ok(RemoteCommand::Revoke("phone-1".to_string())))
        );
        assert_eq!(
            parse_remote_command("/remote unpair my phone"),
            Some(Ok(RemoteCommand::Revoke("my phone".to_string())))
        );
    }

    #[test]
    fn revoke_without_target_is_an_error() {
        let result = parse_remote_command("/remote revoke");
        assert!(matches!(result, Some(Err(msg)) if msg.contains("Usage")));
    }

    #[test]
    fn unknown_subcommand_and_stray_args_are_errors() {
        assert!(matches!(parse_remote_command("/remote wat"), Some(Err(_))));
        assert!(matches!(
            parse_remote_command("/remote on now"),
            Some(Err(_))
        ));
    }

    #[test]
    fn unrelated_commands_are_not_claimed() {
        // `/remote-release` is a different, existing command and must not be
        // swallowed by the `/remote` prefix.
        assert_eq!(parse_remote_command("/remote-release"), None);
        assert_eq!(parse_remote_command("/remotely"), None);
        assert_eq!(parse_remote_command("/resume"), None);
        assert_eq!(parse_remote_command("hello"), None);
    }

    #[test]
    fn status_markdown_guides_setup_when_off() {
        let status = RemoteStatus::from_parts(false, 7643, "127.0.0.1", registry(vec![]));
        let md = status.to_markdown();
        assert!(md.contains("Gateway: **off**"));
        assert!(md.contains("/remote on"));
        assert!(!md.contains("Dial from another machine"));
    }

    #[test]
    fn status_markdown_shows_dial_address_and_devices_when_on() {
        let status = RemoteStatus::from_parts(
            true,
            7643,
            "127.0.0.1",
            registry(vec![device("phone-1", "my phone")]),
        );
        let md = status.to_markdown();
        assert!(md.contains("Gateway: **on**"));
        assert!(md.contains("`127.0.0.1:7643`"));
        assert!(md.contains("my phone"));
        assert!(
            !md.contains("/remote pair\n"),
            "should not nag when a device exists"
        );
    }

    #[test]
    fn wildcard_bind_warns_about_exposure() {
        let status = RemoteStatus::from_parts(true, 7643, "0.0.0.0", registry(vec![]));
        assert!(status.to_markdown().contains("trusted network"));

        let loopback = RemoteStatus::from_parts(true, 7643, "127.0.0.1", registry(vec![]));
        assert!(!loopback.to_markdown().contains("trusted network"));
    }

    #[test]
    fn explicit_bind_address_is_used_verbatim_as_dial_host() {
        let status = RemoteStatus::from_parts(true, 7643, "100.64.1.5", registry(vec![]));
        assert_eq!(status.dial_address(), "100.64.1.5:7643");
        assert!(!status.host_unknown);
    }

    #[test]
    fn wildcard_bind_flags_unknown_reachable_host() {
        assert!(is_wildcard_bind("0.0.0.0"));
        assert!(is_wildcard_bind("::"));
        assert!(!is_wildcard_bind("127.0.0.1"));
    }

    #[test]
    fn spaced_code_groups_six_digits() {
        let invite = PairingInvite {
            code: "082641".to_string(),
            dial_address: "host:7643".to_string(),
            uri: String::new(),
            gateway_enabled: true,
        };
        assert_eq!(invite.spaced_code(), "082 641");
    }

    /// Environment-mutating tests must not run concurrently with each other.
    /// These tests mutate the process-global `JCODE_HOME`, so they must
    /// serialize against *every* other test that does, not just against each
    /// other. A module-private lock only excluded the three tests below, which
    /// left them racing the config and provider suites: a test here would swap
    /// `JCODE_HOME` out from under a provider test mid-assertion, failing it
    /// intermittently whenever the two modules happened to interleave.
    fn lock_env() -> crate::storage::TestEnvGuard {
        crate::storage::lock_test_env()
    }

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
    }

    impl HomeGuard {
        fn new(config_body: &str) -> Self {
            let temp = tempfile::tempdir().expect("temp dir");
            std::fs::write(temp.path().join("config.toml"), config_body).expect("write config");
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp.path());
            crate::config::Config::invalidate_cache();
            Self {
                previous,
                _temp: temp,
            }
        }

        fn config_text(&self) -> String {
            std::fs::read_to_string(self._temp.path().join("config.toml")).expect("read config")
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(prev) => crate::env::set_var("JCODE_HOME", prev),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }

    /// Toggling the gateway rewrites the whole config file, so the real risk is
    /// silently dropping a setting the user cares about. Assert an unrelated
    /// value survives the round trip.
    #[test]
    fn enabling_the_gateway_preserves_unrelated_config() {
        let _lock = lock_env();
        let home = HomeGuard::new(
            "[gateway]\nenabled = false\nport = 7643\nbind_addr = \"0.0.0.0\"\n\n\
             [compaction]\nlookahead_turns = 9\n",
        );

        assert_eq!(
            set_gateway_enabled(true).expect("enable"),
            ToggleOutcome::Changed { enabled: true }
        );

        let written = home.config_text();
        assert!(written.contains("enabled = true"), "{written}");

        let reloaded = crate::config::Config::load();
        assert!(reloaded.gateway.enabled);
        assert_eq!(reloaded.gateway.port, 7643);
        assert_eq!(
            reloaded.compaction.lookahead_turns, 9,
            "unrelated config must survive the rewrite: {written}"
        );
    }

    /// A no-op toggle should not claim a restart is needed.
    #[test]
    fn toggling_to_the_current_value_reports_unchanged() {
        let _lock = lock_env();
        let _home = HomeGuard::new("[gateway]\nenabled = false\nport = 7643\n");

        assert_eq!(
            set_gateway_enabled(false).expect("no-op disable"),
            ToggleOutcome::Unchanged { enabled: false }
        );
    }

    /// Revoking must remove only the requested device and persist the result.
    #[test]
    fn revoke_removes_only_the_named_device() {
        let _lock = lock_env();
        let _home = HomeGuard::new("[gateway]\nenabled = true\n");

        registry(vec![
            device("phone-1", "my phone"),
            device("laptop-2", "laptop"),
        ])
        .save()
        .expect("seed registry");

        assert_eq!(revoke_device("my phone").expect("revoke"), vec!["my phone"]);

        let remaining = DeviceRegistry::load();
        assert_eq!(remaining.devices.len(), 1);
        assert_eq!(remaining.devices[0].id, "laptop-2");

        assert!(
            revoke_device("nobody").expect("revoke missing").is_empty(),
            "revoking an unknown device should report no matches"
        );
    }
}
