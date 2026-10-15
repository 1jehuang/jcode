//! Fresh, fail-closed permission checks for explicit background microphone use.
//!
//! logind's LockedHint is cooperative: a locker must publish its state. This
//! cannot establish that an arbitrary compositor/locker is actually unlocked.
//! Call before each press and periodically during capture, canceling on false.
//!
//! macOS and Windows: the OS itself withholds registered global hotkeys
//! (Carbon RegisterEventHotKey, Win32 RegisterHotKey) from a locked session or
//! secure desktop, and the host is the only edge source there, so no extra
//! session query is made.
#[cfg(target_os = "linux")]
use std::{future::Future, task::Poll, time::Duration};

#[cfg(target_os = "linux")]
const CHECK_TIMEOUT: Duration = Duration::from_millis(200);

/// No shell commands, blocking D-Bus calls, property cache, or UI activation.
#[cfg(target_os = "linux")]
pub async fn permitted() -> bool {
    let check = async { check_session().await.unwrap_or(false) };
    within(check, async_io::Timer::after(CHECK_TIMEOUT)).await
}

#[cfg(not(target_os = "linux"))]
pub async fn permitted() -> bool {
    true
}

#[cfg(target_os = "linux")]
async fn within(check: impl Future<Output = bool>, deadline: impl Future) -> bool {
    let mut check = std::pin::pin!(check);
    let mut deadline = std::pin::pin!(deadline);
    std::future::poll_fn(|cx| {
        // Expiration wins even if a late reply is available in the same poll.
        if deadline.as_mut().poll(cx).is_ready() {
            Poll::Ready(false)
        } else {
            check.as_mut().poll(cx)
        }
    })
    .await
}

#[cfg(target_os = "linux")]
async fn check_session() -> zbus::Result<bool> {
    let connection = zbus::Connection::system().await?;
    // Prefer the process session. User-service hosts may not belong to one, so
    // allow an explicit inherited session only after validating its owner UID.
    let manager = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let path: zbus::Result<zbus::zvariant::OwnedObjectPath> = manager
        .call("GetSessionByPID", &(std::process::id(),))
        .await;
    let path = match path {
        Ok(path) => path,
        Err(_) => {
            let Ok(id) = std::env::var("XDG_SESSION_ID") else {
                return Ok(false);
            };
            if id.is_empty() {
                return Ok(false);
            }
            manager.call("GetSession", &(id,)).await?
        }
    };
    let session: zbus::Proxy<'_> = zbus::proxy::Builder::new(&connection)
        .destination("org.freedesktop.login1")?
        .path(path)?
        .interface("org.freedesktop.login1.Session")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let (uid, _): (u32, zbus::zvariant::OwnedObjectPath) = session.get_property("User").await?;
    // SAFETY: geteuid has no preconditions.
    if uid != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    let active: bool = session.get_property("Active").await?;
    let remote: bool = session.get_property("Remote").await?;
    let kind: String = session.get_property("Type").await?;
    // Read lock status last, nearest to the authorization decision.
    let locked: bool = session.get_property("LockedHint").await?;
    Ok(allowed(active, locked, remote, &kind))
}

/// X11 sessions are allowed too: evdev capture is display-server independent
/// and the X11 OS pill is a non-focusing override-redirect notification.
#[cfg(target_os = "linux")]
fn allowed(active: bool, locked: bool, remote: bool, kind: &str) -> bool {
    active && !locked && !remote && matches!(kind, "wayland" | "x11")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn only_active_unlocked_local_graphical_session_is_allowed() {
        for active in [false, true] {
            for locked in [false, true] {
                for remote in [false, true] {
                    for kind in ["wayland", "x11", "tty", "", "unspecified"] {
                        assert_eq!(
                            allowed(active, locked, remote, kind),
                            active && !locked && !remote && (kind == "wayland" || kind == "x11")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn timeout_and_unknown_results_fail_closed() {
        async_io::block_on(async {
            assert!(!within(std::future::pending(), std::future::ready(())).await);
            assert!(!within(std::future::ready(true), std::future::ready(())).await);
            assert!(!within(std::future::ready(false), std::future::pending::<()>()).await);
            assert!(within(std::future::ready(true), std::future::pending::<()>()).await);
        });
    }
}
