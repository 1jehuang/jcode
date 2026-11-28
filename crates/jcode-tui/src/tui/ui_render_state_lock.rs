//! Test-only render-state lock: serializes tests that touch process-global
//! render state, reentrant per thread.

/// The one lock guarding process-global render state in tests.
///
/// Render snapshots, scroll metrics, flicker history, and prompt positions all
/// live in process globals, so *every* test that renders must serialize on the
/// same mutex. Two separate helpers previously each defined their own private
/// lock, which serialized nothing between them and produced failures that
/// appeared only under parallelism (same root cause as issue #593). Both now
/// delegate here.
///
/// Lock hierarchy: tests needing both `storage::lock_test_env` and this lock
/// must acquire the environment lock first. App construction resets render
/// state while an environment-scoping helper may already hold that lock.
/// Taking environment after render would deadlock against that reset.
///
/// Reentrant per thread: a call from a thread that already holds the lock
/// returns a no-op `Nested` guard instead of deadlocking, so production entry
/// points like `ui::draw` (which now take this lock in test builds) can be
/// invoked freely from inside a locked test.
#[cfg(test)]
pub(crate) fn render_state_test_lock() -> RenderStateTestGuard {
    if render_state_lock_held() {
        return RenderStateTestGuard::Nested;
    }
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    // Global lock order is env lock -> render lock: rendering tests touch env
    // (config, JCODE_HOME) while holding the render lock, and env-taking tests
    // render. Acquiring env first removes the AB-BA cycle that hung ui::tests.
    let env_guard = crate::storage::lock_test_env();
    let guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    RENDER_STATE_LOCK_HELD.with(|held| held.set(true));
    RenderStateTestGuard::Outer {
        _guard: guard,
        _env_guard: env_guard,
    }
}

/// Guard for [`render_state_test_lock`].
///
/// `Outer` owns the mutex and records ownership on this thread so a nested
/// `clear_test_render_state_for_tests` can tell it is already inside the lock
/// instead of deadlocking on it. `Nested` is returned to a caller that already
/// holds the lock; dropping it must leave the flag set because the outer owner
/// still holds it.
#[cfg(test)]
pub(crate) enum RenderStateTestGuard {
    Outer {
        _guard: std::sync::MutexGuard<'static, ()>,
        // Held for the render guard's whole lifetime: releasing env earlier
        // would let a sibling take env and wait on render while this thread
        // still holds render, reopening the AB-BA window. Drops after
        // `_guard` (fields drop in declaration order), so the LIFO
        // env->render order holds on release too.
        _env_guard: crate::storage::TestEnvGuard,
    },
    Nested,
}

#[cfg(test)]
impl Drop for RenderStateTestGuard {
    fn drop(&mut self) {
        if matches!(self, Self::Outer { .. }) {
            RENDER_STATE_LOCK_HELD.with(|held| held.set(false));
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Whether this thread currently holds the render-state lock. Set by
    /// [`render_state_test_lock`]'s guard so nested clears can detect it.
    static RENDER_STATE_LOCK_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn render_state_lock_held() -> bool {
    RENDER_STATE_LOCK_HELD.with(|held| held.get())
}
