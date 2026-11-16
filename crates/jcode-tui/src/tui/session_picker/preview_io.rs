//! Read-only preview failures fall back to uncached/streaming paths.

pub(super) fn preview_result<T, E: std::fmt::Display>(
    result: Result<T, E>,
    operation: &str,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            crate::logging::debug(&format!("Session preview {operation} unavailable: {error}"));
            None
        }
    }
}
