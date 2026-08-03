//! Environment capability probing for login flows.
//!
//! Historically jcode discovered environment limitations *by failing*: it would
//! open a browser that does not exist, wait 60 seconds for a callback that can
//! never arrive, then string-match the resulting English error message in
//! [`crate::auth::login_diagnostics::classify_auth_failure_message`] to guess
//! what went wrong. That works, but it spends the user's first ninety seconds
//! on a flow that was never going to succeed.
//!
//! This module inverts that: probe the environment cheaply and concurrently up
//! front, then *choose* an auth method that can actually work. The probe results
//! are also exactly the right shape for privacy-preserving telemetry, because
//! every field is a three-valued enum with no user data in it.
//!
//! Design note: [`Tri::Unknown`] always biases toward the optimistic path. A
//! wrong `No` is worse than a wrong `Unknown`, because it silently downgrades a
//! user who would have had a perfectly good browser login.

use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// Three-valued capability answer. `Unknown` means "we could not determine
/// this cheaply", and every consumer must treat it as "probably yes".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tri {
    Yes,
    No,
    #[default]
    Unknown,
}

impl Tri {
    /// Closed-vocabulary label, safe to emit in telemetry verbatim.
    pub fn label(self) -> &'static str {
        match self {
            Tri::Yes => "yes",
            Tri::No => "no",
            Tri::Unknown => "unknown",
        }
    }

    /// True unless we positively determined the capability is missing.
    ///
    /// This is the optimistic reading that every decision site should use, so
    /// an inconclusive probe never downgrades a working environment.
    pub fn is_probably(self) -> bool {
        !matches!(self, Tri::No)
    }

    /// True only when we positively determined the capability is missing.
    pub fn is_definitely_not(self) -> bool {
        matches!(self, Tri::No)
    }

    fn from_bool(value: bool) -> Self {
        if value { Tri::Yes } else { Tri::No }
    }
}

/// Capabilities of the machine we are trying to log in from.
///
/// Every field is a `Tri` and nothing here is user data: no paths, hostnames,
/// usernames, or error text. The whole struct can be sent as telemetry as-is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvFacts {
    /// Interactive stdin/stdout: can we prompt at all?
    pub tty: Tri,
    /// A browser can plausibly be launched (a launcher exists and, on Linux, a
    /// display server is present).
    pub browser: Tri,
    /// We can bind a loopback socket for an OAuth callback.
    pub loopback_bind: Tri,
    /// The jcode config directory exists (or can be created) and is writable.
    pub config_writable: Tri,
    /// Running inside a container/SSH/remote shell, where browser-based OAuth
    /// redirects usually land on the wrong machine.
    pub container: Tri,
    /// An HTTP(S) proxy is configured, which changes failure modes enough that
    /// it is worth reporting alongside network errors.
    pub proxy: Tri,
}

impl EnvFacts {
    /// Probe everything cheaply. Target budget is a few milliseconds: every
    /// probe is a syscall or an environment lookup, never a network round-trip.
    ///
    /// Network reachability and clock skew are deliberately excluded here
    /// because they require talking to a provider; they belong to the login
    /// attempt itself, not to a startup probe.
    pub fn probe() -> Self {
        Self {
            tty: probe_tty(),
            browser: probe_browser(),
            loopback_bind: probe_loopback_bind(),
            config_writable: probe_config_writable(),
            container: probe_container(),
            proxy: probe_proxy(),
        }
    }

    /// Probe, and log how long it took. Used at first-run so we can confirm the
    /// budget holds on real machines.
    pub fn probe_timed() -> (Self, Duration) {
        let started = Instant::now();
        let facts = Self::probe();
        (facts, started.elapsed())
    }

    /// The auth method that can actually succeed here, most preferred first.
    ///
    /// This is the table from the design doc. It replaces "try the browser and
    /// see what happens" with a decision we can test exhaustively.
    pub fn preferred_auth_method(&self) -> AuthMethodChoice {
        // A read-only or missing config dir dooms every flow: we would complete
        // the login and then fail to persist it. Say so before wasting a login.
        if self.config_writable.is_definitely_not() {
            return AuthMethodChoice::BlockedConfigUnwritable;
        }
        // Without a TTY there is nobody to prompt, so only non-interactive
        // credential sources can work.
        if self.tty.is_definitely_not() {
            return AuthMethodChoice::ApiKeyNonInteractive;
        }
        // No confirmed browser means a redirect has nowhere to land. A remote
        // or containerized session counts here only when we could not confirm a
        // browser: SSH with X forwarding has a real, working browser, and
        // downgrading that user would be a regression.
        if self.browser.is_definitely_not()
            || (self.container == Tri::Yes && self.browser != Tri::Yes)
        {
            return AuthMethodChoice::DeviceCode;
        }
        if self.loopback_bind.is_definitely_not() {
            return AuthMethodChoice::OAuthPasteCallback;
        }
        AuthMethodChoice::OAuthLoopback
    }
}

/// Which login mechanism to offer, chosen from [`EnvFacts`] rather than
/// discovered by failing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodChoice {
    /// Best case: open a browser, catch the redirect on loopback.
    OAuthLoopback,
    /// Browser works but we cannot bind a callback port; the user pastes the
    /// callback URL back into the terminal.
    OAuthPasteCallback,
    /// No usable browser on this machine (or the browser would open somewhere
    /// else): show a code the user completes on another device.
    DeviceCode,
    /// Nobody to prompt: read an API key from the environment or stdin.
    ApiKeyNonInteractive,
    /// Do not start a login at all; fix the config directory first.
    BlockedConfigUnwritable,
}

impl AuthMethodChoice {
    /// Closed-vocabulary label, safe for telemetry.
    pub fn label(self) -> &'static str {
        match self {
            AuthMethodChoice::OAuthLoopback => "oauth_loopback",
            AuthMethodChoice::OAuthPasteCallback => "oauth_paste_callback",
            AuthMethodChoice::DeviceCode => "device_code",
            AuthMethodChoice::ApiKeyNonInteractive => "api_key_non_interactive",
            AuthMethodChoice::BlockedConfigUnwritable => "blocked_config_unwritable",
        }
    }

    /// What to tell the user *before* they start, so a doomed flow never runs.
    pub fn precondition_message(self) -> Option<&'static str> {
        match self {
            AuthMethodChoice::BlockedConfigUnwritable => Some(
                "jcode cannot write its config directory, so a login could not be saved. Fix directory permissions (or set JCODE_HOME) and try again.",
            ),
            AuthMethodChoice::ApiKeyNonInteractive => Some(
                "This terminal is not interactive. Provide an API key via the provider's environment variable or `jcode provider add --api-key-stdin`.",
            ),
            _ => None,
        }
    }
}

fn probe_tty() -> Tri {
    use std::io::IsTerminal;
    Tri::from_bool(std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
}

fn probe_browser() -> Tri {
    // Explicit override wins: users in odd setups tell us directly.
    if std::env::var("BROWSER").is_ok_and(|value| !value.trim().is_empty()) {
        return Tri::Yes;
    }
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        // `open` / `start` are always present.
        return Tri::Yes;
    }
    // On Linux a launcher without a display server cannot show anything.
    let has_display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()));
    if !has_display {
        return Tri::No;
    }
    match binary_on_path("xdg-open") {
        // A display but no launcher is unusual; stay optimistic rather than
        // downgrading a user who has some other mechanism.
        false => Tri::Unknown,
        true => Tri::Yes,
    }
}

fn probe_loopback_bind() -> Tri {
    // Bind an ephemeral loopback port. If even this fails, a fixed OAuth
    // callback port certainly will, and the user should paste instead.
    Tri::from_bool(TcpListener::bind("127.0.0.1:0").is_ok())
}

fn probe_config_writable() -> Tri {
    let Ok(dir) = crate::storage::jcode_dir() else {
        return Tri::No;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return Tri::No;
    }
    // An actual write is the only honest test: a read-only mount, a full disk,
    // and a permission problem all present differently to a metadata check.
    let probe_path = dir.join(".write-probe");
    match std::fs::write(&probe_path, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe_path);
            Tri::Yes
        }
        Err(_) => Tri::No,
    }
}

fn probe_container() -> Tri {
    // An SSH session is the most common "the browser opens on the wrong
    // machine" case, and it is unambiguous.
    if std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok() {
        return Tri::Yes;
    }
    for key in [
        "CODESPACES",
        "GITPOD_WORKSPACE_ID",
        "DEVCONTAINER",
        "REMOTE_CONTAINERS",
    ] {
        if std::env::var(key).is_ok_and(|value| !value.trim().is_empty()) {
            return Tri::Yes;
        }
    }
    if cfg!(target_os = "linux") && std::path::Path::new("/.dockerenv").exists() {
        return Tri::Yes;
    }
    // WSL forwards browser launches to Windows and works fine, so it is
    // deliberately not treated as a container here.
    Tri::No
}

fn probe_proxy() -> Tri {
    let configured = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()));
    Tri::from_bool(configured)
}

fn binary_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build facts for a test case without probing the host.
    fn facts(tty: Tri, browser: Tri, loopback: Tri, writable: Tri, container: Tri) -> EnvFacts {
        EnvFacts {
            tty,
            browser,
            loopback_bind: loopback,
            config_writable: writable,
            container,
            proxy: Tri::No,
        }
    }

    #[test]
    fn unknown_is_always_optimistic() {
        // The whole probe rests on this: an inconclusive answer must never be
        // the reason a user gets a worse login flow.
        assert!(Tri::Unknown.is_probably());
        assert!(Tri::Yes.is_probably());
        assert!(!Tri::No.is_probably());
        assert!(!Tri::Unknown.is_definitely_not());

        let all_unknown = facts(
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
        );
        assert_eq!(
            all_unknown.preferred_auth_method(),
            AuthMethodChoice::OAuthLoopback,
            "a fully inconclusive probe must still offer the best flow"
        );
    }

    #[test]
    fn method_selection_covers_the_documented_table() {
        // Ordered most-blocking first, matching `preferred_auth_method`.
        let cases = [
            (
                facts(Tri::Yes, Tri::Yes, Tri::Yes, Tri::No, Tri::No),
                AuthMethodChoice::BlockedConfigUnwritable,
            ),
            (
                facts(Tri::No, Tri::Yes, Tri::Yes, Tri::Yes, Tri::No),
                AuthMethodChoice::ApiKeyNonInteractive,
            ),
            (
                // Container with a *confirmed* browser (e.g. SSH with X
                // forwarding): keep the best flow rather than downgrading a
                // user whose browser demonstrably works.
                facts(Tri::Yes, Tri::Yes, Tri::Yes, Tri::Yes, Tri::Yes),
                AuthMethodChoice::OAuthLoopback,
            ),
            (
                // Container with no confirmed browser: a redirect would land on
                // the wrong machine, so carry a code across instead.
                facts(Tri::Yes, Tri::Unknown, Tri::Yes, Tri::Yes, Tri::Yes),
                AuthMethodChoice::DeviceCode,
            ),
            (
                facts(Tri::Yes, Tri::No, Tri::Yes, Tri::Yes, Tri::No),
                AuthMethodChoice::DeviceCode,
            ),
            (
                facts(Tri::Yes, Tri::Yes, Tri::No, Tri::Yes, Tri::No),
                AuthMethodChoice::OAuthPasteCallback,
            ),
            (
                facts(Tri::Yes, Tri::Yes, Tri::Yes, Tri::Yes, Tri::No),
                AuthMethodChoice::OAuthLoopback,
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(
                facts.preferred_auth_method(),
                expected,
                "unexpected method for {facts:?}"
            );
        }
    }

    #[test]
    fn every_env_fact_combination_yields_a_usable_choice() {
        // Exhaustive over the whole 3^5 fact space: no combination may produce
        // a choice with no path forward. `BlockedConfigUnwritable` counts as a
        // path because it carries a precondition message telling the user what
        // to fix.
        let tri = [Tri::Yes, Tri::No, Tri::Unknown];
        let mut seen = std::collections::BTreeSet::new();
        for tty in tri {
            for browser in tri {
                for loopback in tri {
                    for writable in tri {
                        for container in tri {
                            let f = facts(tty, browser, loopback, writable, container);
                            let choice = f.preferred_auth_method();
                            seen.insert(choice.label());
                            if choice == AuthMethodChoice::BlockedConfigUnwritable {
                                assert!(
                                    choice.precondition_message().is_some(),
                                    "a blocking choice must explain itself"
                                );
                            }
                        }
                    }
                }
            }
        }
        // Every authored method must be reachable under some environment.
        // An unreachable method is dead code that will rot.
        for method in [
            AuthMethodChoice::OAuthLoopback,
            AuthMethodChoice::OAuthPasteCallback,
            AuthMethodChoice::DeviceCode,
            AuthMethodChoice::ApiKeyNonInteractive,
            AuthMethodChoice::BlockedConfigUnwritable,
        ] {
            assert!(
                seen.contains(method.label()),
                "{} is unreachable under every environment",
                method.label()
            );
        }
    }

    #[test]
    fn a_normal_desktop_keeps_the_browser_flow() {
        // Regression guard for the probe's integration with
        // `auth::browser_suppressed`: a plain desktop must never be downgraded
        // out of the browser flow by the probe. Only DeviceCode and
        // ApiKeyNonInteractive suppress the browser there.
        let desktop = facts(Tri::Yes, Tri::Yes, Tri::Yes, Tri::Yes, Tri::No);
        assert_eq!(
            desktop.preferred_auth_method(),
            AuthMethodChoice::OAuthLoopback
        );

        // And the most pessimistic reading of an inconclusive probe still keeps
        // the browser, because Unknown is optimistic everywhere.
        let inconclusive = facts(
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
            Tri::Unknown,
        );
        assert!(matches!(
            inconclusive.preferred_auth_method(),
            AuthMethodChoice::OAuthLoopback | AuthMethodChoice::OAuthPasteCallback
        ));
    }

    #[test]
    fn probe_is_cheap_and_carries_no_user_data() {
        let (facts, elapsed) = EnvFacts::probe_timed();
        assert!(
            elapsed < Duration::from_millis(500),
            "probe budget exceeded: {elapsed:?}"
        );
        // The serialized form is what telemetry would send. It must contain
        // nothing but the closed Tri vocabulary.
        let json = serde_json::to_string(&facts).expect("facts serialize");
        for value in json
            .split(['{', '}', ',', ':', '"'])
            .filter(|s| !s.is_empty())
        {
            assert!(
                matches!(
                    value,
                    "tty"
                        | "browser"
                        | "loopback_bind"
                        | "config_writable"
                        | "container"
                        | "proxy"
                        | "yes"
                        | "no"
                        | "unknown"
                ),
                "unexpected token {value:?} in telemetry payload {json}"
            );
        }
    }

    #[test]
    fn loopback_bind_succeeds_on_a_normal_machine() {
        // Sanity check that the probe measures something real rather than
        // always answering the same way.
        assert_eq!(probe_loopback_bind(), Tri::Yes);
    }
}
