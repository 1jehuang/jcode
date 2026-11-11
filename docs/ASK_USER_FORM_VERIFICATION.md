# Ask-user form specification verification

Specification: toolkit-research repository,
`projects/jcode/plans/done/ASK_USER_FORM.md` (2026-11-11).

The specification lives outside this repository. It defines the model input,
ACP form schema and result contract as well as the TUI interaction and lifecycle.

## Deviations found and corrected

| Spec behavior | Deviation | Regression test |
| --- | --- | --- |
| Typing on Other opens its editor and preserves composed input | Printable input outside an editor was swallowed | `typing_on_other_keeps_the_first_composed_character_in_the_answer` |
| Shift+Tab navigates backward | Tab with SHIFT moved forward on question tabs; BackTab worked | `shift_tab_key_encoding_moves_back_without_changing_answers` |
| Active text editors remain visible, including in narrow terminals | Notes opened from preview remained covered by the preview | `opening_notes_from_preview_shows_the_text_being_edited` |

All fixes are in the fork-owned form module. No protocol or upstream hooks changed.
No substantial implementation deviation was identified requiring delegation.

## Focused coverage

The new generated-form round trip executes the actual AskUserTool with a fake
form-capable broker attachment, drives its generated schema through TUI input,
submits through broker validation and decodes the resulting model-facing values.
It checks single-select Other, multi-select boundaries, Unicode, commas, quotes,
newlines and notes without constraining the order of a selection set.

Additional tests exercise the 8192-code-point limit through submitted content
for both notes and Other (including preservation after rejected paste), and
schema-changed reconnects that must discard stale answers and drafts.
The panel-height helper test was folded into real rendering checks, including
the 11/12-row boundary; the expected cap no longer comes from the implementation
being tested.

Existing tests retained cover input validation and reserved labels, Other
normalization, previews, escaping, broker validation, registration and explicit
allowlists, headless/late-attach/hibernation behavior, keyboard navigation,
grapheme edits, preview scrolling, narrow rendering, focus precedence,
acknowledgement ownership, submission races, reconnect and session cleanup.
Schema-field assertions were retained because they specify the public ACP
interface used by clients, rather than private implementation structure.

This is automated contract verification, not a fresh interactive runtime
acceptance pass. Real terminal input encoding, live provider calls and daemon
reloads still require the private-socket runtime procedure in the specification.

## Validation on 2026-11-11

On atlas, using the shared Cargo target cache and `nice -n 10`:

- `cargo test -p jcode-tui --lib tui::ask_form::tests:: -- --test-threads=2`:
  54 passed.
- `cargo test -p jcode-tui --lib tui::app::remote::server_events::ask_form_tests:: -- --test-threads=2`:
  16 passed.
- The new composed-input test failed against the original implementation
  before its fix. All final targeted tests pass; `git diff --check` passes.
