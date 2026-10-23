pub(super) fn message(size: &str, language: &str, copied: bool) -> String {
    let (prefix, suffix) = if copied {
        ("", " copied")
    } else {
        ("Could not copy ", "")
    };
    format!("Image size: {size} · {prefix}{language} code{suffix}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagram_clipboard_notice_matches_success_and_failure() {
        for size in ["fit", "large", "full"] {
            for (copied, language, expected) in [
                (true, "D2", "D2 code copied"),
                (false, "D2", "Could not copy D2 code"),
                (true, "Mermaid", "Mermaid code copied"),
                (false, "Mermaid", "Could not copy Mermaid code"),
            ] {
                assert_eq!(
                    super::message(size, language, copied),
                    format!("Image size: {size} · {expected}")
                );
            }
        }
    }
}
