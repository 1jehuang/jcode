//! @file mention composer integration: suggestion generation, tab
//! completion, and chip upkeep for the input UI.
//! Split out of state_ui_input_helpers.rs (PR 1889).

use std::path::Path;

use super::input::file_chip_matches_input;
use super::{App, PathBuf};

impl App {
    /// Generate file-mention suggestions from the @file query at the cursor.
    pub(super) fn file_mention_suggestions(&self) -> Vec<(String, &'static str)> {
        let (_at_pos, query) = match crate::tui::ui::input_ui::extract_at_query_for_cursor(
            &self.input,
            self.cursor_pos,
        ) {
            Some(q) => q,
            None => return Vec::new(),
        };
        self.file_mention_suggestions_for_query(&query)
    }

    /// File-mention suggestions for an explicit raw query.
    /// `file_mention_suggestions` derives the query from the composer; this
    /// variant lets the Tab-cycle re-derive the candidate list from its saved
    /// base query instead of the narrowed post-completion input (greptile A5).
    fn file_mention_suggestions_for_query(&self, query: &str) -> Vec<(String, &'static str)> {
        // File mentions resolve against the local filesystem, so they must
        // not run over SSH remotes: `session.working_dir` is the server's
        // path, and reading it locally would embed the client's files (or
        // wrong files at a coinciding path) into the remote prompt
        // (greptile A2).
        if crate::tui::is_ssh_remote() {
            return Vec::new();
        }

        let cwd = self
            .session
            .working_dir
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new("."));

        let cache = &mut *self.file_mention_cache.borrow_mut();
        cache.check_refresh(cwd);

        let candidates = cache.candidates(query);

        // First use: index is still building → show a hint with live progress.
        // Empty candidate lists after a completed build mean "no matches", not
        // "still building". The hint row is a sentinel, not a real suggestion:
        // the accept path below rejects it so Enter never inserts it as a chip.
        if candidates.is_empty() && cache.is_initial_building() {
            let indexed = cache.indexed_so_far();
            let hint = if indexed > 0 {
                format!("⏳ Building file index... {indexed} files")
            } else {
                "⏳ Building file index...".to_string()
            };
            return vec![(hint, "first use takes a few seconds")];
        }

        // Built index with zero hits: say so instead of flashing an empty
        // popover. Same sentinel rules as the building hint (⏳ prefix):
        // never selectable, never acceptable.
        if candidates.is_empty() && !query.trim_start_matches('/').is_empty() {
            return vec![(
                "⏳ No matching files".to_string(),
                "check the spelling or type a path",
            )];
        }

        // Group into Recent / Files sections (design §14).
        let mut grouped: Vec<(String, &'static str)> = Vec::new();
        let mut recent: Vec<(String, &'static str)> = Vec::new();
        let mut files: Vec<(String, &'static str)> = Vec::new();

        for c in candidates {
            let display = if c.is_directory {
                if c.path.ends_with('/') {
                    c.path.to_string()
                } else {
                    format!("{}/", c.path)
                }
            } else {
                c.path.to_string()
            };
            let desc: &'static str = if c.is_likely_binary {
                "binary"
            } else if c.is_recent {
                "recent"
            } else {
                ""
            };
            if c.is_recent {
                recent.push((display, desc));
            } else {
                files.push((display, desc));
            }
        }

        // Section header entries use empty cmd as sentinel.
        if !recent.is_empty() {
            grouped.push((String::new(), "── Recent ──"));
            grouped.extend(recent);
        }
        if !files.is_empty() {
            grouped.push((String::new(), "── Files ──"));
            grouped.extend(files);
        }
        grouped
    }

    /// Advance idx forward past any section-header entries (empty cmd).
    pub(super) fn skip_section_headers_down(
        idx: &mut usize,
        suggestions: &[(String, &'static str)],
    ) {
        let len = suggestions.len();
        if len == 0 {
            return;
        }
        let initial = *idx;
        while suggestions[*idx].0.is_empty() {
            *idx = (*idx + 1) % len;
            if *idx == initial {
                break;
            }
        }
    }

    /// Retreat idx backward past any section-header entries (empty cmd).
    pub(super) fn skip_section_headers_up(idx: &mut usize, suggestions: &[(String, &'static str)]) {
        let len = suggestions.len();
        if len == 0 {
            return;
        }
        let initial = *idx;
        while suggestions[*idx].0.is_empty() {
            // wrapping_sub(1) % len only wraps to len-1 when len is a power of
            // two; use rem_euclid so idx=0 steps to len-1 for any len.
            *idx = (*idx + len - 1).rem_euclid(len);
            if *idx == initial {
                break;
            }
        }
    }

    /// File-mention tab completion: common-prefix then cycle.
    pub(super) fn autocomplete_file_mention(&mut self) -> bool {
        let (at_pos, query) = match crate::tui::ui::input_ui::extract_at_query_for_cursor(
            &self.input,
            self.cursor_pos,
        ) {
            Some(q) => q,
            None => return false,
        };

        let suggestions = self.file_mention_suggestions();
        if suggestions.is_empty() {
            return false;
        }

        // Filter out section headers (empty cmd) and the "building index"
        // hint row — neither is a real completion target.
        let paths: Vec<&str> = suggestions
            .iter()
            .filter(|(s, _)| !s.is_empty() && !s.starts_with('⏳'))
            .map(|(s, _)| s.trim_end_matches('/'))
            .collect();
        // A popover with only sentinel rows (headers, hint) has nothing to
        // complete: cycling would divide by zero and completing would insert
        // junk. Treat it like "no suggestions".
        if paths.is_empty() {
            return false;
        }

        // Step 1: longest common prefix (if it's longer than the current query).
        if let Some(common) = common_prefix(&paths)
            && common.len() > query.len()
            && common.starts_with(&query)
            && let Some((new_input, new_cursor)) =
                crate::tui::ui::input_ui::tab_complete_at(&self.input, self.cursor_pos, &common)
        {
            self.remember_input_undo_state();
            self.input = new_input;
            self.cursor_pos = new_cursor;
            self.tab_completion_state = None;
            return true;
        }

        // Step 2: cycle through candidates. The cycle is anchored to the
        // saved base query, not the current input: after Tab completes
        // "@src" to "@src/a.txt", re-deriving candidates from the narrowed
        // input degenerates the cycle (the next Tab would query "src/a.txt"
        // and re-offer the same file). The slash-command cycle keeps its
        // base for the same reason; this mirrors it (greptile A5).
        // A cycle saved from another mention (or a caret move that landed the
        // cursor outside the mention the cycle started on) must not leak into
        // this one: drop the stale state and restart the cycle from the
        // current mention (greptile "Tab uses another mention").
        let cycle_state_valid = self
            .tab_completion_state
            .as_ref()
            .is_some_and(|(_, _, saved_at)| *saved_at == at_pos);
        if !cycle_state_valid {
            self.tab_completion_state = None;
        }

        let cycle_list: Vec<String> = match self.tab_completion_state.as_ref() {
            Some((base, _, _)) => {
                let base_paths: Vec<String> = self
                    .file_mention_suggestions_for_query(base)
                    .into_iter()
                    .filter(|(s, _)| !s.is_empty() && !s.starts_with('⏳'))
                    .map(|(s, _)| s.trim_end_matches('/').to_string())
                    .collect();
                if base_paths.is_empty() {
                    paths.iter().map(|s| s.to_string()).collect()
                } else {
                    base_paths
                }
            }
            None => paths.iter().map(|s| s.to_string()).collect(),
        };

        let (_base, next_index, _at) = self
            .tab_completion_state
            .get_or_insert_with(|| (query.clone(), 0, at_pos));
        let cycle_idx = *next_index % cycle_list.len();
        *next_index = cycle_idx + 1;

        let chosen = cycle_list[cycle_idx].clone();
        if let Some((new_input, new_cursor)) =
            crate::tui::ui::input_ui::tab_complete_at(&self.input, self.cursor_pos, &chosen)
        {
            self.remember_input_undo_state();
            self.input = new_input;
            self.cursor_pos = new_cursor;
            return true;
        }

        false
    }

    /// Remove file chips whose paths no longer appear in the input text.
    ///
    /// Matching is path-boundary aware (shared with chip deletion and
    /// highlight rendering via `input::file_chip_matches_input`): a chip only
    /// survives while its path appears as a complete path token, so a chip
    /// like `bag` is dropped once the text only contains `char_bag.rs`. The
    /// old plain-substring keep rule kept chips alive mid-token.
    pub(super) fn prune_orphan_chips(&mut self) {
        let text = self.input.clone();
        self.file_chips
            .retain(|chip_path| file_chip_matches_input(&text, chip_path));
    }
}

/// Longest common prefix of a list of strings.
fn common_prefix(strings: &[&str]) -> Option<String> {
    let first = strings.first()?;
    let first_chars: Vec<char> = first.chars().collect();
    let mut end = first_chars.len();
    for s in strings.iter().skip(1) {
        end = first_chars
            .iter()
            .zip(s.chars())
            .take(end)
            .take_while(|(a, b)| **a == *b)
            .count();
        if end == 0 {
            return None;
        }
    }
    // `end` counts chars; collect from chars so multi-byte UTF-8 boundaries are
    // never sliced mid-sequence (a byte slice here used to panic on paths like
    // "éx"/"éy" whose shared prefix ends inside a multi-byte char).
    Some(first_chars[..end].iter().collect())
}

/// Resolve a relative or absolute path into an absolute `PathBuf`.
#[allow(dead_code)]
fn resolve_path(path: &str, cwd: Option<&Path>) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let cwd = cwd.unwrap_or_else(|| Path::new("."));
    let resolved = cwd.join(p);
    resolved.canonicalize().unwrap_or(resolved)
}

#[cfg(test)]
mod file_mention_helper_tests {
    use super::*;

    #[test]
    fn common_prefix_basic() {
        assert_eq!(
            common_prefix(&["src/cli/startup.rs", "src/cli/selfdev.rs"]),
            Some("src/cli/s".to_string())
        );
    }

    /// Regression: a byte-count prefix used to slice mid-UTF-8-char and panic
    /// when the shared prefix ended inside a multi-byte character.
    #[test]
    fn common_prefix_multibyte_never_panics() {
        // Shared prefix "é" (2 bytes) followed by differing ASCII tails.
        assert_eq!(common_prefix(&["éx", "éy"]), Some("é".to_string()));
        // Shared multi-byte prefix of several chars.
        assert_eq!(
            common_prefix(&["café/x.rs", "café/y.rs"]),
            Some("café/".to_string())
        );
        // Divergence inside a multi-byte char: prefix must cut at the char
        // boundary (é shares fully, è differs from é's second char... they
        // share only 'c','a','f').
        assert_eq!(
            common_prefix(&["café.rs", "cafe.rs"]),
            Some("caf".to_string())
        );
        // All-ASCII sanity.
        assert_eq!(common_prefix(&["ab.rs", "ac.rs"]), Some("a".to_string()));
    }

    #[test]
    fn common_prefix_single() {
        assert_eq!(
            common_prefix(&["src/main.rs"]),
            Some("src/main.rs".to_string())
        );
    }

    #[test]
    fn common_prefix_none() {
        assert_eq!(common_prefix(&["a/b", "c/d"]), None);
    }

    #[test]
    fn common_prefix_empty() {
        assert_eq!(common_prefix(&[]), None);
    }

    #[test]
    fn resolve_path_relative() {
        let cwd = Path::new("/home/user/project");
        let result = resolve_path("src/main.rs", Some(cwd));
        assert!(result.ends_with("src/main.rs"));
        assert!(result.starts_with("/"));
    }

    #[test]
    fn resolve_path_absolute() {
        let result = resolve_path("/etc/hosts", None);
        assert_eq!(result, PathBuf::from("/etc/hosts"));
    }

    fn suggestion_rows(cmds: &[&str]) -> Vec<(String, &'static str)> {
        cmds.iter()
            .map(|c| ((*c).to_string(), ""))
            .collect::<Vec<_>>()
    }

    /// Up-skip from idx=0 must wrap to the last row for any list length, not
    /// just powers of two (regression: `wrapping_sub(1) % len` computed
    /// `usize::MAX % len`, landing on arbitrary rows for non-power-of-2 len).
    #[test]
    fn section_header_skip_up_wraps_correctly_for_non_power_of_two_len() {
        use super::App;

        // len=3: [header, file, file]
        let rows = suggestion_rows(&["", "a.rs", "b.rs"]);
        let mut idx = 0;
        App::skip_section_headers_up(&mut idx, &rows);
        assert_eq!(idx, 2, "idx=0 on a header must wrap to the last row");

        // len=5 with a header at the end: stepping back must land on row 3.
        let rows = suggestion_rows(&["a.rs", "b.rs", "c.rs", "d.rs", ""]);
        let mut idx = 4;
        App::skip_section_headers_up(&mut idx, &rows);
        assert_eq!(idx, 3);

        // len=6, header at 0 and 5: wrap twice and land on a real row.
        let rows = suggestion_rows(&["", "a.rs", "b.rs", "c.rs", "d.rs", ""]);
        let mut idx = 5;
        App::skip_section_headers_up(&mut idx, &rows);
        assert_eq!(idx, 4);
    }

    #[test]
    fn section_header_skip_down_and_up_never_land_on_header() {
        use super::App;

        // Down-skip from a header in the middle of the list advances past it.
        let rows = suggestion_rows(&["a.rs", "", "b.rs"]);
        let mut idx = 1;
        App::skip_section_headers_down(&mut idx, &rows);
        assert_eq!(idx, 2);

        // Down-skip on a non-header row is a no-op (the caller moves the
        // selection first; the helper only advances off headers).
        let rows = suggestion_rows(&["a.rs", "b.rs", ""]);
        let mut idx = 1;
        App::skip_section_headers_down(&mut idx, &rows);
        assert_eq!(idx, 1);

        let mut idx = 2;
        App::skip_section_headers_down(&mut idx, &rows);
        assert_eq!(idx, 0, "header at end wraps down to row 0");

        // All-header list: both helpers must terminate without panicking.
        let rows = suggestion_rows(&["", ""]);
        let mut down = 0;
        App::skip_section_headers_down(&mut down, &rows);
        assert_eq!(down, 0);
        let mut up = 0;
        App::skip_section_headers_up(&mut up, &rows);
        assert_eq!(up, 0);
    }
}
