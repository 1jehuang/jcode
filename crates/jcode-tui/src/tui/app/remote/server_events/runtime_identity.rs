pub(super) fn allow_runtime_identity_mismatch() -> bool {
    std::env::var_os("JCODE_ALLOW_SERVER_VERSION_MISMATCH").is_some()
}

/// Parse a jcode version string into an orderable `(major, minor, patch)`, but
/// only for *clean release* builds.
///
/// Dev/dirty builds share a base semver and cannot be ordered against each other
/// or against releases (issue #277/#291: a self-dev / branched daemon must never
/// be force-downgraded just because its version string differs). So we refuse to
/// classify anything carrying a `-dev` or `dirty` marker as an orderable version
/// and return `None`, leaving such daemons to the existing `server_has_update`
/// (mtime-directional) path.
pub(super) fn parse_release_semver(version: &str) -> Option<(u32, u32, u32)> {
    let lower = version.trim().to_ascii_lowercase();
    if lower.contains("-dev") || lower.contains("dirty") {
        return None;
    }
    // Take the leading token, e.g. "v0.17.0 (d741696f)" -> "0.17.0".
    let token = lower
        .split([' ', '(', ')', ','])
        .next()
        .unwrap_or(&lower)
        .trim();
    let token = token.strip_prefix('v').unwrap_or(token);
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True when the connected server reports a clean release version strictly older
/// than this client's own clean release version.
///
/// This is the missing client-side staleness signal behind issue #295: a server
/// old enough to predate the self-reported staleness machinery reports
/// `server_has_update: None`, so it can never tell us it is stale and the client
/// happily attaches to it (then a `set_route`-shaped request explodes against the
/// ancient protocol). We detect that case independently here.
///
/// Gated on clean release semvers on BOTH sides, so dev/dirty/self-dev daemons
/// (which cannot be ordered) are never affected.
pub(super) fn server_release_is_older_than_client(
    server_version: Option<&str>,
    client_version: &str,
) -> bool {
    let Some(server) = server_version.and_then(parse_release_semver) else {
        return false;
    };
    let Some(client) = parse_release_semver(client_version) else {
        return false;
    };
    server < client
}

/// Decide whether to defer applying remote session state because the server we
/// attached to is not running the binary we expect.
///
/// Precedence:
/// - The client independently measured the server's release version as strictly
///   older than its own clean release version -> defer. This wins even over the
///   server's own `server_has_update: Some(false)` self-report, because a stale
///   long-lived daemon legitimately reports "no newer binary to reload into"
///   (its `shared-server` channel still points at its own old build) while the
///   client can plainly see it is an older release. Trusting the server here is
///   exactly what left "current client, stale server" stuck (the daemon's reload
///   decision runs old code that can never drag itself forward). The newer
///   client is authoritative, so it defers and repairs the channel before
///   reloading.
/// - `Some(true)`: the server self-reported a newer binary on disk -> defer.
/// - `Some(false)`: the server is new enough to self-assess and found nothing
///   newer to reload into, AND the client could not prove it is older -> trust
///   it, do not fight it with a forced reload.
/// - `None`: the server is too old to self-report. Fall back to our own
///   client-side release-version comparison, which is the only signal that can
///   catch a pre-self-heal daemon.
pub(super) fn should_defer_history_for_runtime_identity_with_allow(
    server_has_update: Option<bool>,
    client_detected_stale: bool,
    allow_mismatch: bool,
) -> bool {
    if allow_mismatch {
        return false;
    }
    // A client-proven-older server always wins: never let an old daemon's
    // (locally correct but globally wrong) "no update" self-report veto the
    // client's own release-order comparison.
    if client_detected_stale {
        return true;
    }
    match server_has_update {
        Some(true) => true,
        Some(false) => false,
        None => false,
    }
}

/// The client's own version string, used for release-staleness comparison.
///
/// Production always reads the compiled-in build metadata. A test-only env
/// override exists so the end-to-end `handle_server_event` path can be exercised
/// from a dev/dirty test binary (whose real version would otherwise be
/// unorderable and short-circuit the comparison).
pub(super) fn client_release_version() -> String {
    if (cfg!(test) || cfg!(debug_assertions))
        && let Some(v) = std::env::var_os("JCODE_TEST_CLIENT_VERSION_OVERRIDE")
    {
        return v.to_string_lossy().into_owned();
    }
    jcode_build_meta::version().to_string()
}

pub(super) fn should_defer_history_for_runtime_identity(
    server_has_update: Option<bool>,
    server_version: Option<&str>,
) -> bool {
    let client_detected_stale =
        server_release_is_older_than_client(server_version, &client_release_version());
    should_defer_history_for_runtime_identity_with_allow(
        server_has_update,
        client_detected_stale,
        allow_runtime_identity_mismatch(),
    )
}
