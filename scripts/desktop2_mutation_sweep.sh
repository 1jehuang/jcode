#!/usr/bin/env bash
# Mutation sweep for jcode-desktop2.
#
# A test suite is only worth its claims if it fails when the code is wrong.
# This applies a list of targeted mutations (each a plausible, realistic bug),
# runs the suite, and reports any that SURVIVE. A survivor is a coverage hole:
# behavior nothing asserts.
#
#   scripts/desktop2_mutation_sweep.sh          # fast tests only
#   scripts/desktop2_mutation_sweep.sh --gpu    # also run pixel tests
set -uo pipefail

cd "$(dirname "$0")/.."
crate=crates/jcode-desktop2
backup="$(mktemp -d)"
cp -r "$crate/src" "$backup/src"
restore() { rm -rf "$crate/src"; cp -r "$backup/src" "$crate/src"; }
trap 'restore; rm -rf "$backup"' EXIT

gpu=0
[ "${1:-}" = "--gpu" ] && gpu=1

survivors=0
caught=0

# Each mutation: label | file | python-quoted find | replace
run_mutation() {
    local label="$1" file="$2" find="$3" replace="$4"
    python3 - "$crate/$file" "$find" "$replace" <<'PYEOF'
import pathlib, sys
path, find, replace = sys.argv[1], sys.argv[2], sys.argv[3]
p = pathlib.Path(path)
s = p.read_text()
if find not in s:
    print("SKIP-NOMATCH")
    raise SystemExit(3)
p.write_text(s.replace(find, replace, 1))
PYEOF
    local patched=$?
    if [ "$patched" -eq 3 ]; then
        printf '  %-52s %s\n' "$label" "SKIPPED (pattern not found)"
        restore
        return
    fi

    local failures=0
    if cargo test --profile selfdev -p jcode-desktop2 --quiet >/dev/null 2>&1; then
        if [ "$gpu" -eq 1 ]; then
            if ! cargo test --profile selfdev -p jcode-desktop2 --quiet -- --ignored \
                >/dev/null 2>&1; then
                failures=1
            fi
        fi
    else
        failures=1
    fi

    if [ "$failures" -eq 1 ]; then
        printf '  %-52s %s\n' "$label" "caught"
        caught=$((caught + 1))
    else
        printf '  %-52s %s\n' "$label" "SURVIVED <- coverage hole"
        survivors=$((survivors + 1))
    fi
    restore
}

echo "mutation sweep (gpu=$gpu)"

# --- editor: buffer and cursor ---
run_mutation "insert appends instead of at cursor" src/editor.rs \
    'self.text.insert_str(self.cursor, &s);' \
    'self.text.push_str(&s);'
run_mutation "undo never records" src/editor.rs \
    'fn snapshot(&mut self) {' \
    'fn snapshot(&mut self) { if true { return; }'
run_mutation "word-back stops at whitespace" src/editor.rs \
    'while pos > 0 {
            let prev = self.prev_boundary(pos);
            let ch = self.text[prev..].chars().next().unwrap_or('"'"' '"'"');
            if ch.is_whitespace() {
                break;
            }
            pos = prev;
        }' \
    ''
run_mutation "kill-to-end ignores line bounds" src/editor.rs \
    'let end = self.line_end(self.cursor);' \
    'let end = self.text.len();'
run_mutation "home goes to buffer start" src/editor.rs \
    'self.cursor = self.line_start(self.cursor);
        self.anchor = None;
    }

    pub fn move_end' \
    'self.cursor = 0;
        self.anchor = None;
    }

    pub fn move_end'
run_mutation "typing does not replace selection" src/editor.rs \
    '// Typing over a selection replaces it, like any normal input box.
        self.delete_selection();' \
    ''
run_mutation "motion keeps a stale selection" src/editor.rs \
    'self.cursor = self.prev_boundary(self.cursor);
        self.anchor = None;
    }' \
    'self.cursor = self.prev_boundary(self.cursor);
    }'
run_mutation "line motion never reports an edge" src/editor.rs \
    'if line == 0 {
                    return false;
                }' \
    'if line == 0 {
                    return true;
                }'
run_mutation "newlines are stripped like other controls" src/editor.rs \
    "*c == '\\n' || !c.is_control()" \
    '!c.is_control()'

# --- caret ---
run_mutation "caret never blinks" src/caret.rs \
    'phase < (BLINK_PERIOD.as_millis() as u64) / 2' \
    'true'
run_mutation "caret is not solid while typing" src/caret.rs \
    'if elapsed < SOLID_AFTER_INPUT {
            return true;
        }' \
    ''
run_mutation "blink schedules no wake" src/caret.rs \
    'pub fn next_toggle_at(&self, now: Instant) -> Option<Instant> {' \
    'pub fn next_toggle_at(&self, now: Instant) -> Option<Instant> { if true { return None; }'

# --- keymap ---
run_mutation "escape quits the app" src/keymap.rs \
    'NamedKey::Escape => Some(Action::Cancel),' \
    'NamedKey::Escape => Some(Action::InterruptOrQuit),'
run_mutation "shift+enter submits" src/keymap.rs \
    'Some(if shift {
                Action::InsertNewline
            } else {
                Action::Submit
            })' \
    'Some(Action::Submit)'
run_mutation "plain letters become shortcuts" src/keymap.rs \
    'if cmd {
                return match ch {' \
    'if true {
                return match ch {'
run_mutation "shift+arrow moves instead of selecting" src/keymap.rs \
    '(true, false, _) => Action::ExtendLeft,' \
    '(true, false, _) => Action::MoveLeft,'

# --- layout ---
run_mutation "composer does not grow" src/layout.rs \
    'let extra_lines = lines.clamp(1, COMPOSER_MAX_LINES) - 1;' \
    'let extra_lines = 0; let _ = lines;'
run_mutation "measure column is unbounded" src/layout.rs \
    '.clamp(120.0, MEASURE)' \
    '.max(120.0)'
run_mutation "hairline is not one physical pixel" src/layout.rs \
    '1.0 / self.scale' \
    '1.0'
run_mutation "wrap budget far too generous" src/layout.rs \
    'pub const MONOSPACE_ADVANCE_RATIO: f64 = 0.6;' \
    'pub const MONOSPACE_ADVANCE_RATIO: f64 = 0.25;'

# --- wrap ---
run_mutation "no wrapping at all" src/wrap.rs \
    'pub fn wrap(text: &str, max_chars: usize) -> Vec<Row> {' \
    'pub fn wrap(text: &str, max_chars: usize) -> Vec<Row> {
    if true { let _ = max_chars; return vec![Row { start: 0, end: text.len(), hard_break: false }]; }'
run_mutation "wrapping breaks mid-word always" src/wrap.rs \
    '.filter(|(index, _)| *index > 0)' \
    '.filter(|(index, _)| *index > usize::MAX / 2)'
run_mutation "row lookup always returns row 0" src/wrap.rs \
    'pub fn row_col_at(rows: &[Row], source: &str, offset: usize) -> (usize, usize) {' \
    'pub fn row_col_at(rows: &[Row], source: &str, offset: usize) -> (usize, usize) {
    if true { let _ = (rows, source, offset); return (0, 0); }'

# --- window state ---
run_mutation "geometry is not sanitized" src/window_state.rs \
    'pub fn sanitized(self) -> Self {' \
    'pub fn sanitized(self) -> Self { if true { return self; }
        #[allow(unreachable_code)]'
run_mutation "saving is never throttled" src/window_state.rs \
    'Some((_, previous)) if previous == self.sanitized() => false,' \
    'Some((_, previous)) if previous == self.sanitized() => true,'

# --- theme ---
run_mutation "selection band matches the text ink" src/theme.rs \
    'selection: Color::from_rgb8(0xd8, 0xd8, 0xd8),' \
    'selection: Color::from_rgb8(0x11, 0x11, 0x11),'
run_mutation "faint is darker than body text" src/theme.rs \
    'faint: Color::from_rgb8(0x99, 0x99, 0x99),
            rule: Color::from_rgb8(0xcc, 0xcc, 0xcc),' \
    'faint: Color::from_rgb8(0x00, 0x00, 0x00),
            rule: Color::from_rgb8(0xcc, 0xcc, 0xcc),'

# --- app wiring ---
run_mutation "shift+enter inserts a space" src/main.rs \
    "Action::InsertNewline => self.model.editor.insert_char('\\n')," \
    "Action::InsertNewline => self.model.editor.insert_char(' '),"
run_mutation "scrolling does not clamp" src/main.rs \
    'let max = self.transcript_lines().saturating_sub(visible);
        self.scroll = (self.scroll + lines).min(max);' \
    'let _ = visible;
        self.scroll += lines;'
run_mutation "submit does not return to the tail" src/main.rs \
    '// Submitting jumps back to the live tail; otherwise the reply streams
        // in off-screen.
        self.model.scroll = 0;' \
    ''
run_mutation "click extends instead of placing" src/main.rs \
    'self.model.editor.place_cursor(offset);' \
    'self.model.editor.extend_to(offset);'
run_mutation "drag ignores pointer movement" src/main.rs \
    'if let Some(offset) = self.composer_offset_at(x, y) {
            self.model.editor.extend_to(offset);' \
    'if let Some(offset) = self.composer_offset_at(x, y) {
            let _ = offset;'
run_mutation "hit-testing ignores the wrapped row" src/main.rs \
    'let row = rows[first + offset_row];' \
    'let row = rows[0];'
run_mutation "pointer shape never changes" src/main.rs \
    'let wanted = if self.in_composer(x, y) {' \
    'let wanted = if false {'
run_mutation "frame ignores wrapping" src/main.rs \
    'let rows = model.composer_rows(probe.composer_char_budget());
        layout::Frame::with_composer_lines(size, scale, rows.len())' \
    'let _ = probe;
        layout::Frame::with_composer_lines(size, scale, 1)'
run_mutation "ctrl+c quits with unsent text" src/main.rs \
    '} else if !self.model.editor.is_empty() {
                    self.model.editor.clear();
                } else {
                    return false;
                }' \
    '} else {
                    return false;
                }'

# --- text / rendering ---
run_mutation "text laid out at 1x on HiDPI" src/text.rs \
    '.ranged_builder(&mut self.fonts, text, scale32, true);
        Self::push_defaults(&mut builder, style);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(None);' \
    '.ranged_builder(&mut self.fonts, text, 1.0, true);
        Self::push_defaults(&mut builder, style);
        let mut layout: Layout<Brush> = builder.build(text);
        layout.break_all_lines(None);'
run_mutation "hit test trims trailing whitespace" src/text.rs \
    'f64::from(layout.full_width()) / scale' \
    'f64::from(layout.width()) / scale'
run_mutation "caret drawn at a fixed x" src/scene.rs \
    'let offset = text.measure_width(prefix, prompt_style, scale);' \
    'let offset = 0.0f64;'
run_mutation "caret drawn on the first row" src/scene.rs \
    'let top = row_y(row.max(first)) - 1.0;' \
    'let top = row_y(first) - 1.0;'
run_mutation "selection band is never drawn" src/scene.rs \
    'if let Some((sel_start, sel_end)) = model.editor.selection() {' \
    'if let Some((sel_start, sel_end)) = None::<(usize, usize)> {'

echo
echo "caught: $caught   survived: $survivors"
if [ "$survivors" -gt 0 ]; then
    echo "Survivors are behavior no test asserts. Add a test, then re-run."
    exit 1
fi
echo "mutation sweep: every mutation was caught"
