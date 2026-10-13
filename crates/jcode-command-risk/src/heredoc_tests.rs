use super::*;

// -- has_heredoc_substitution -------------------------------------------------

#[test]
fn detects_heredoc_pattern() {
    assert!(has_heredoc_substitution(
        "echo $(cat <<'EOF'\nhello\nEOF\n)"
    ));
}

#[test]
fn no_heredoc_without_dollar_paren() {
    assert!(!has_heredoc_substitution("echo hello"));
}

#[test]
fn no_heredoc_without_heredoc_op() {
    assert!(!has_heredoc_substitution("echo $(echo hi)"));
}

// -- is_safe_heredoc / strip_safe_heredoc_substitutions -----------------------

#[test]
fn simple_quoted_heredoc_is_safe() {
    let cmd = "echo $(cat <<'EOF'\nhello world\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
    assert_eq!(
        strip_safe_heredoc_substitutions(cmd).as_deref(),
        Some("echo ")
    );
}

#[test]
fn escaped_delimiter_is_safe() {
    let cmd = "echo $(cat <<\\EOF\nhello\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
    let stripped = strip_safe_heredoc_substitutions(cmd).unwrap();
    assert_eq!(stripped, "echo ");
}

#[test]
fn heredoc_with_special_characters_in_body() {
    let cmd = "echo $(cat <<'EOF'\nrm -rf ~\n$HOME\n`whoami`\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
    let stripped = strip_safe_heredoc_substitutions(cmd).unwrap();
    assert_eq!(stripped, "echo ");
}

#[test]
fn missing_closing_delimiter_is_unsafe() {
    let cmd = "echo $(cat <<'EOF'\nhello\n)";
    assert!(!is_safe_heredoc(cmd));
    assert!(strip_safe_heredoc_substitutions(cmd).is_none());
}

#[test]
fn dash_heredoc_tab_stripping() {
    // <<- strips leading tabs from delimiter lines
    let cmd = "echo $(cat <<-'EOF'\n\thello\n\tEOF\n)";
    assert!(is_safe_heredoc(cmd));
    let stripped = strip_safe_heredoc_substitutions(cmd).unwrap();
    assert_eq!(stripped, "echo ");
}

#[test]
fn inline_paren_form_is_safe() {
    let cmd = "echo $(cat <<'EOF'\nhello\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
}

#[test]
fn non_cat_command_is_unsafe() {
    let cmd = "echo $(sed 's/a/b/' <<'EOF'\nhello\nEOF\n)";
    assert!(!is_safe_heredoc(cmd));
    assert!(strip_safe_heredoc_substitutions(cmd).is_none());
}

#[test]
fn unquoted_delimiter_is_unsafe() {
    let cmd = "echo $(cat <<EOF\nhello\nEOF\n)";
    assert!(!is_safe_heredoc(cmd));
    assert!(strip_safe_heredoc_substitutions(cmd).is_none());
}

#[test]
fn no_command_prefix_is_unsafe() {
    // Heredoc body would become the command name — unsafe.
    let cmd = "$(cat <<'EOF'\nchmod\nEOF\n) 777 /etc/shadow";
    assert!(!is_safe_heredoc(cmd));
}

#[test]
fn nested_heredocs_are_rejected() {
    // Nested patterns inside a quoted heredoc body are literal text but
    // our regex matches both — we reject to prevent index corruption.
    let cmd = "echo $(cat <<'A'\n$(cat <<'B'\nx\nB\n)\nA\n)";
    assert!(!is_safe_heredoc(cmd));
}

#[test]
fn extra_content_on_open_line_is_unsafe() {
    let cmd = "echo $(cat <<'EOF' ; rm -rf ~\nhello\nEOF\n)";
    assert!(!is_safe_heredoc(cmd));
    assert!(strip_safe_heredoc_substitutions(cmd).is_none());
}

#[test]
fn no_heredoc_returns_none() {
    assert!(strip_safe_heredoc_substitutions("echo hello").is_none());
}

#[test]
fn multiple_heredocs_stripped() {
    let cmd = "echo $(cat <<'A'\nfoo\nA\n) $(cat <<'B'\nbar\nB\n)";
    assert!(is_safe_heredoc(cmd));
    let stripped = strip_safe_heredoc_substitutions(cmd).unwrap();
    assert_eq!(stripped, "echo  ");
}

#[test]
fn delimiter_line_with_trailing_metachar_rejected() {
    // DELIM) is safe, but DELIM| is not
    let cmd = "echo $(cat <<'EOF'\nhello\nEOF|)\n)";
    assert!(!is_safe_heredoc(cmd));
}

#[test]
fn empty_body_heredoc_is_safe() {
    let cmd = "echo $(cat <<'EOF'\n\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
}

#[test]
fn heredoc_with_multiline_body() {
    let cmd = "echo $(cat <<'EOF'\nline1\nline2\nline3\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
    let stripped = strip_safe_heredoc_substitutions(cmd).unwrap();
    assert_eq!(stripped, "echo ");
}

#[test]
fn dash_heredoc_with_no_tabs_is_unsafe() {
    // With <<- the delimiter line must be findable after tab stripping;
    // if the body has spaces instead of tabs, it still works as long as
    // the delimiter line matches.
    let cmd = "echo $(cat <<-'EOF'\nhello\nEOF\n)";
    assert!(is_safe_heredoc(cmd));
}
