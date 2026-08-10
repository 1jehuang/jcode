//! Tokenizer tests. The tokenizer's job is to make quoting and chaining
//! non-bypasses, so those are the cases that matter.

use super::*;

fn texts(command: &str) -> Vec<String> {
    tokenize(command).into_iter().map(|t| t.text).collect()
}

#[test]
fn quoting_does_not_hide_a_target() {
    // All three must yield the same target word.
    for command in [r#"rm -rf $HOME"#, r#"rm -rf "$HOME""#, r#"rm -rf '$HOME'"#] {
        assert_eq!(texts(command), vec!["rm", "-rf", "$HOME"], "{command}");
    }
}

#[test]
fn backslash_escapes_are_resolved() {
    assert_eq!(
        texts(r"rm -rf /home/my\ files"),
        vec!["rm", "-rf", "/home/my files"]
    );
}

#[test]
fn chained_commands_split_into_separate_segments() {
    let segments = split_segments("echo hi && rm -rf /tmp/a; rm -rf /tmp/b");
    assert_eq!(segments.len(), 3, "{segments:#?}");
    assert_eq!(segments[1][0].text, "rm");
    assert_eq!(segments[2][0].text, "rm");
}

#[test]
fn piped_commands_are_separate_segments() {
    let segments = split_segments("find . -type f | xargs rm -rf");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[1][0].text, "xargs");
}

#[test]
fn truncating_redirect_target_is_marked_but_append_is_not() {
    let clobber = tokenize("echo x > important.conf");
    assert!(
        clobber
            .iter()
            .any(|t| t.is_truncating_redirect_target && t.text == "important.conf")
    );

    let append = tokenize("echo x >> log.txt");
    assert!(
        !append.iter().any(|t| t.is_truncating_redirect_target),
        "append does not truncate and must not be flagged"
    );
}

#[test]
fn recursive_flag_detected_in_bundles_and_long_form() {
    let cases = [
        ("-rf", true),
        ("-fr", true),
        ("-R", true),
        ("--recursive", true),
        ("-f", false),
        ("--force", false),
        ("-i", false),
    ];
    for (flag, expected) in cases {
        let token = Token::word(flag);
        assert_eq!(token.is_recursive_flag(), expected, "{flag}");
    }
}

#[test]
fn basename_strips_the_path_so_absolute_invocations_match() {
    assert_eq!(Token::word("/bin/rm").basename(), "rm");
    assert_eq!(Token::word("/usr/bin/shred").basename(), "shred");
}

#[test]
fn tokenizing_is_total_and_never_panics() {
    // Malformed input must degrade gracefully rather than crash the tool call.
    for command in [
        "",
        "   ",
        "'",
        "\"",
        "\\",
        ">",
        "&&",
        "rm -rf \"unterminated",
    ] {
        let _ = split_segments(command);
    }
}

#[test]
fn parentheses_split_subshell_and_command_substitution_bodies() {
    let segments = split_segments("x=$(rm -rf ~); (echo ok)");
    let texts: Vec<Vec<_>> = segments
        .into_iter()
        .map(|segment| segment.into_iter().map(|token| token.text).collect())
        .collect();
    assert_eq!(
        texts,
        vec![vec!["x=$"], vec!["rm", "-rf", "~"], vec!["echo", "ok"]]
    );
}
