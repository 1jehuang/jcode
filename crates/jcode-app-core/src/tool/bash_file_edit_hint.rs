const FILE_EDIT_HINT: &str = "Note: this command edits files in place. Next time use `edit` \
(exact replacements, all-or-nothing, pass `edits` for several) or `apply_patch` \
(multi-file patches). They fail loudly on a missed match \
instead of silently writing nothing, and show a reviewable diff.";

/// Detect shell commands that rewrite source files in place, where a missed
/// match silently does nothing. The command still runs, the agent is nudged.
pub(super) fn file_edit_hint(command: &str) -> Option<&'static str> {
    let compact: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let sed_in_place = compact.split(['|', ';', '&']).any(|segment| {
        let mut words = segment.split_whitespace();
        words.next() == Some("sed")
            && words.any(|word| {
                word == "--in-place"
                    || word.starts_with("--in-place=")
                    || (word.starts_with('-') && !word.starts_with("--") && word[1..].contains('i'))
            })
    });
    let perl_in_place = compact
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair[0] == "perl" && pair[1].starts_with('-') && pair[1].contains('i'));
    let script_rewrite = (compact.contains("python") || compact.contains("node "))
        && (compact.contains(".replace(") || compact.contains("re.sub("))
        && (compact.contains(",'w')")
            || compact.contains(",\"w\")")
            || compact.contains(", 'w')")
            || compact.contains(", \"w\")")
            || compact.contains("write_text(")
            || compact.contains("writeFileSync("));
    (sed_in_place || perl_in_place || script_rewrite).then_some(FILE_EDIT_HINT)
}
