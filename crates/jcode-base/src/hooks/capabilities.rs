//! Hook and agent capability tokens advertised by `jcode version --json`.
//!
//! Frameworks that integrate through hooks read `hook_capabilities` to pick
//! the adapter a given jcode build can support, instead of comparing version
//! numbers. Each feature adds its own token here in the change that ships it.

/// Capability tokens, kept sorted so the output is stable.
const TOKENS: &[&str] = &["project_hooks", "project_trust"];

/// Sorted, de-duplicated capability tokens supported by this build.
pub fn hook_capabilities() -> Vec<&'static str> {
    let mut tokens: Vec<&'static str> = TOKENS.to_vec();
    tokens.sort_unstable();
    tokens.dedup();
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_sorted_and_unique() {
        let tokens = hook_capabilities();
        let mut expected = tokens.clone();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(tokens, expected);
    }
}
