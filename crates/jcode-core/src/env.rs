use std::ffi::{OsStr, OsString};

/// Mutate the process environment for jcode runtime configuration.
///
/// Rust 2024 makes environment mutation unsafe because it can race with
/// concurrent environment access in foreign code. jcode intentionally mutates
/// process-local env vars to coordinate provider/runtime bootstrap before or
/// during task execution. We centralize that unsafety here so call sites remain
/// auditable.
pub fn set_var<K, V>(key: K, value: V)
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    // SAFETY: jcode treats these mutations as process-global configuration.
    // They are a pre-existing design choice used throughout startup, auth,
    // provider bootstrap, tests, and self-dev flows. Centralizing the unsafe
    // operation here makes the Rust 2024 requirement explicit without
    // scattering unsafe blocks across hundreds of call sites.
    unsafe {
        std::env::set_var(key, value);
    }
}

/// Remove a process environment variable used by jcode runtime configuration.
pub fn remove_var<K>(key: K)
where
    K: AsRef<OsStr>,
{
    // SAFETY: see `set_var` above; this is the corresponding centralized
    // removal operation for the same process-global configuration surface.
    unsafe {
        std::env::remove_var(key);
    }
}

/// Sets or removes one environment variable for the guard's lifetime, then
/// restores the value it replaced (removing the variable if it was unset),
/// also when the scope unwinds from a panic.
///
/// Tests that redirect `JCODE_HOME` must restore the caller's value rather
/// than removing it: a removal points every later test in the process at the
/// real `~/.jcode`, where they write sessions and build manifests.
#[must_use = "the previous value is restored when the guard is dropped"]
pub struct ScopedVar {
    key: OsString,
    previous: Option<OsString>,
}

impl ScopedVar {
    pub fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        let key = key.as_ref().to_os_string();
        let previous = std::env::var_os(&key);
        set_var(&key, value);
        Self { key, previous }
    }

    pub fn remove(key: impl AsRef<OsStr>) -> Self {
        let key = key.as_ref().to_os_string();
        let previous = std::env::var_os(&key);
        remove_var(&key);
        Self { key, previous }
    }
}

impl Drop for ScopedVar {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => set_var(&self.key, value),
            None => remove_var(&self.key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedVar;

    #[test]
    fn scoped_var_restores_the_replaced_value_and_unset_state() {
        const KEY: &str = "JCODE_CORE_SCOPED_VAR_TEST";
        super::set_var(KEY, "outer");
        {
            let _inner = ScopedVar::set(KEY, "inner");
            assert_eq!(std::env::var(KEY).as_deref(), Ok("inner"));
            {
                let _removed = ScopedVar::remove(KEY);
                assert!(std::env::var_os(KEY).is_none());
            }
            assert_eq!(std::env::var(KEY).as_deref(), Ok("inner"));
        }
        assert_eq!(std::env::var(KEY).as_deref(), Ok("outer"));
        super::remove_var(KEY);
        {
            let _set = ScopedVar::set(KEY, "temporary");
        }
        assert!(std::env::var_os(KEY).is_none());
    }

    #[test]
    fn scoped_var_restores_on_panic() {
        const KEY: &str = "JCODE_CORE_SCOPED_VAR_PANIC_TEST";
        super::set_var(KEY, "outer");
        let result = std::panic::catch_unwind(|| {
            let _guard = ScopedVar::set(KEY, "inner");
            panic!("unwind through the guard");
        });
        assert!(result.is_err());
        assert_eq!(std::env::var(KEY).as_deref(), Ok("outer"));
        super::remove_var(KEY);
    }
}
