//! Helpers for talking to the public GitHub REST API.
//!
//! Unauthenticated GitHub API requests share a 60 req/hour per-IP bucket
//! with every other tool on the machine (and everything behind the same
//! NAT/VPN), so anonymous metadata lookups intermittently fail with 403
//! even when the user's own authenticated quota (5000 req/hour) is unused.
//! Callers that hit `api.github.com` for public read-only metadata (release
//! lookups, commit SHAs) should attach the token from
//! [`github_public_api_token`] when one is available.

use std::sync::OnceLock;

/// Resolve a GitHub token for authenticated public API access.
///
/// Order: `GH_TOKEN`/`GITHUB_TOKEN` env vars, then a `gh auth token`
/// subprocess lookup cached once per process (including a cached miss, so
/// the subprocess is spawned at most once). Returns `None` when no token
/// is available; callers should proceed unauthenticated in that case.
///
/// Only send this token to `api.github.com` public read endpoints.
pub fn github_public_api_token() -> Option<String> {
    let env_token = ["GH_TOKEN", "GITHUB_TOKEN"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
    });
    if env_token.is_some() {
        return env_token;
    }

    static GH_CLI_TOKEN: OnceLock<Option<String>> = OnceLock::new();
    GH_CLI_TOKEN.get_or_init(gh_cli_token).clone()
}

fn gh_cli_token() -> Option<String> {
    if !crate::auth::command_exists("gh") {
        return None;
    }
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}
