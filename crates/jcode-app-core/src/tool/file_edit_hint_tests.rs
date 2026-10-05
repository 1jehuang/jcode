//! Tests for `#[path]`-attributed module `file_edit_hint_tests` of `bash.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::file_edit_hint;

#[test]
fn flags_in_place_edits() {
    for command in [
        "sed -i 's/a/b/' src/main.rs",
        "cd x && sed -Ei 's/a/b/g' f.rs",
        "sed --in-place=.bak 's/a/b/' f",
        "perl -pi -e 's/a/b/' f.rs",
        "python3 - <<'EOF'\ns=open(p).read()\ns=s.replace('a','b')\nopen(p,'w').write(s)\nEOF",
        "python3 -c \"import pathlib;p=pathlib.Path('f');p.write_text(p.read_text().replace('a','b'))\"",
    ] {
        assert!(file_edit_hint(command).is_some(), "{command}");
    }
}

#[test]
fn ignores_read_only_commands() {
    for command in [
        "sed -n '1,20p' f.rs",
        "sed 's/a/b/' f.rs",
        "grep -i foo f.rs",
        "cargo test -p jcode-app-core",
        "python3 -c \"print('a'.replace('a','b'))\"",
        "git diff --ignore-space-change",
    ] {
        assert!(file_edit_hint(command).is_none(), "{command}");
    }
}
