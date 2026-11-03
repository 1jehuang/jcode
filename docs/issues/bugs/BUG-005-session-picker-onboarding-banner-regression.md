# [BUG-005] Session-picker onboarding banner regression

**ID:** BUG-005
**Category:** bug
**Severity:** medium
**File:** crates/kcode-tui/src/tui/session_picker_tests.rs:1428
**Status:** open

## Description
CI run 38118557038 reported a session-picker onboarding banner regression at
`crates/kcode-tui/src/tui/session_picker_tests.rs:1428`. The assertion at that line checks the
blank-session action stays "secondary in the bottom-right"; the rendered layout no longer satisfies
the expected placement/width, so the onboarding/blank-session banner is wrong (content overflows or
is positioned incorrectly).

## Evidence
```
crates/kcode-tui/src/tui/session_picker_tests.rs:1428:
    assert!(
        start_y >= buffer.area.height as usize - 3 && start_x > width / 2 && start_end + 4 >= width,
        "blank-session action should stay secondary in the bottom-right: {lines:#?}"
    );
```

## Suggested fix
Re-run `cargo test -p kcode-tui --lib session_picker` and inspect the `{lines:#?}` dump. Determine
whether the banner content changed width (regressing `start_end + 4 >= width`) or the layout anchor
moved. Fix the render or update the expectation if the redesign is intentional, but treat width
overflow as a real bug.

## Effort
- S (< 30m)
