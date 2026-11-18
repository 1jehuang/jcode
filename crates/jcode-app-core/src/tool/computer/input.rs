//! Synthetic mouse + keyboard input via Core Graphics CGEvents.
//!
//! This is the *visible* control path: events go to the shared HID stream, so
//! they move the real cursor and type into the focused app. Background control
//! lives in `ax.rs` instead.

use super::keys;
use anyhow::{Context, Result, bail};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use std::thread::sleep;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Button {
    Left,
    Right,
}

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| {
        anyhow::anyhow!(
            "failed to create CGEventSource. Grant Accessibility permission (run the `setup` action)."
        )
    })
}

fn post(event: CGEvent) {
    event.post(CGEventTapLocation::HID);
}

/// Current cursor position in global (top-left origin) screen points.
pub fn current_cursor() -> Result<CGPoint> {
    let src = source()?;
    let evt = CGEvent::new(src).map_err(|_| anyhow::anyhow!("failed to read cursor position"))?;
    Ok(evt.location())
}

pub fn move_to(x: f64, y: f64) -> Result<()> {
    let src = source()?;
    let evt = CGEvent::new_mouse_event(
        src,
        CGEventType::MouseMoved,
        CGPoint::new(x, y),
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("failed to create mouse-move event"))?;
    post(evt);
    Ok(())
}

pub fn click(x: Option<f64>, y: Option<f64>, button: Button, count: u32) -> Result<CGPoint> {
    let point = match (x, y) {
        (Some(x), Some(y)) => CGPoint::new(x, y),
        _ => current_cursor()?,
    };
    let (down, up, cg_button) = match button {
        Button::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        Button::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
    };

    let src = source()?;
    let mv = CGEvent::new_mouse_event(src, CGEventType::MouseMoved, point, cg_button)
        .map_err(|_| anyhow::anyhow!("failed to create move event"))?;
    post(mv);
    sleep(Duration::from_millis(10));

    for i in 1..=count {
        let src_d = source()?;
        let down_evt = CGEvent::new_mouse_event(src_d, down, point, cg_button)
            .map_err(|_| anyhow::anyhow!("failed to create mouse-down event"))?;
        if count > 1 {
            down_evt.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i as i64);
        }
        post(down_evt);

        let src_u = source()?;
        let up_evt = CGEvent::new_mouse_event(src_u, up, point, cg_button)
            .map_err(|_| anyhow::anyhow!("failed to create mouse-up event"))?;
        if count > 1 {
            up_evt.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i as i64);
        }
        post(up_evt);
        sleep(Duration::from_millis(20));
    }
    Ok(point)
}

pub fn drag(from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<()> {
    let from = CGPoint::new(from_x, from_y);
    let to = CGPoint::new(to_x, to_y);

    let src = source()?;
    let down = CGEvent::new_mouse_event(src, CGEventType::LeftMouseDown, from, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("failed to create drag-down event"))?;
    post(down);
    sleep(Duration::from_millis(30));

    let steps = 10;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let p = CGPoint::new(from_x + (to_x - from_x) * t, from_y + (to_y - from_y) * t);
        let src_m = source()?;
        let mv =
            CGEvent::new_mouse_event(src_m, CGEventType::LeftMouseDragged, p, CGMouseButton::Left)
                .map_err(|_| anyhow::anyhow!("failed to create drag-move event"))?;
        post(mv);
        sleep(Duration::from_millis(15));
    }

    let src_u = source()?;
    let up = CGEvent::new_mouse_event(src_u, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("failed to create drag-up event"))?;
    post(up);
    Ok(())
}

pub fn scroll(x: Option<f64>, y: Option<f64>, dx: i32, dy: i32) -> Result<()> {
    if let (Some(x), Some(y)) = (x, y) {
        move_to(x, y)?;
        sleep(Duration::from_millis(10));
    }
    let src = source()?;
    let evt = CGEvent::new_scroll_event(src, ScrollEventUnit::PIXEL, 2, dy, dx, 0)
        .map_err(|_| anyhow::anyhow!("failed to create scroll event"))?;
    post(evt);
    Ok(())
}

/// Type a UTF-8 string as a single synthesized keyboard event (Unicode payload),
/// layout-independent. Goes to the focused app.
pub fn type_text(text: &str) -> Result<()> {
    let src = source()?;
    let down = CGEvent::new_keyboard_event(src, 0, true)
        .map_err(|_| anyhow::anyhow!("failed to create keyboard event"))?;
    down.set_string(text);
    post(down);

    let src_up = source()?;
    let up = CGEvent::new_keyboard_event(src_up, 0, false)
        .map_err(|_| anyhow::anyhow!("failed to create keyboard event"))?;
    up.set_string(text);
    post(up);
    Ok(())
}

/// Parse a chord like "cmd+shift+t" into (modifier flags, main keycode).
pub fn parse_chord(chord: &str) -> Result<(CGEventFlags, u16)> {
    let mut flags = CGEventFlags::CGEventFlagNull;
    let mut keycode: Option<u16> = None;
    for raw in chord.split('+') {
        let part = raw.trim().to_lowercase();
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "cmd" | "command" | "meta" | "super" => flags |= CGEventFlags::CGEventFlagCommand,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "alt" | "opt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "fn" => flags |= CGEventFlags::CGEventFlagSecondaryFn,
            other => {
                if keycode.is_some() {
                    bail!("key chord '{chord}' has more than one non-modifier key");
                }
                keycode = Some(
                    keys::keycode_for(other)
                        .with_context(|| format!("unknown key '{other}' in chord '{chord}'"))?,
                );
            }
        }
    }
    let code = keycode.with_context(|| format!("chord '{chord}' has no main key"))?;
    Ok((flags, code))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChordKeyEvent {
    code: u16,
    down: bool,
    modifier: bool,
    flags: CGEventFlags,
}

fn chord_event_plan(flags: CGEventFlags, code: u16, baseline: CGEventFlags) -> Vec<ChordKeyEvent> {
    let modifiers = [
        (CGEventFlags::CGEventFlagCommand, 0x37),
        (CGEventFlags::CGEventFlagShift, 0x38),
        (CGEventFlags::CGEventFlagAlternate, 0x3A),
        (CGEventFlags::CGEventFlagControl, 0x3B),
        (CGEventFlags::CGEventFlagSecondaryFn, 0x3F),
    ];
    // Only release modifiers this chord introduced. A physically held modifier
    // or an intentional key_hold must still be held when the chord finishes.
    let introduced = flags & !baseline;
    let mut active = baseline;
    let mut plan = Vec::new();
    for (flag, modifier_code) in modifiers {
        if introduced.contains(flag) {
            active.insert(flag);
            plan.push(ChordKeyEvent {
                code: modifier_code,
                down: true,
                modifier: true,
                flags: active,
            });
        }
    }
    for down in [true, false] {
        plan.push(ChordKeyEvent {
            code,
            down,
            modifier: false,
            flags: active,
        });
    }
    for (flag, modifier_code) in modifiers.into_iter().rev() {
        if introduced.contains(flag) {
            active.remove(flag);
            plan.push(ChordKeyEvent {
                code: modifier_code,
                down: false,
                modifier: true,
                flags: active,
            });
        }
    }
    plan
}

static KEY_CHORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_chord_transaction<T>(inject: impl FnOnce() -> Result<T>) -> Result<T> {
    // Concurrent chords must not adopt each other's transient modifiers as
    // their baseline. This does not lock out physical keyboard activity.
    let _guard = KEY_CHORD_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("keyboard chord lock poisoned by interrupted injection"))?;
    inject()
}

fn prepare_and_post_chord<E>(
    plan: &[ChordKeyEvent],
    mut prepare: impl FnMut(&ChordKeyEvent) -> Result<E>,
    mut emit: impl FnMut(E),
) -> Result<()> {
    // Creation can fail. Finish it before posting anything to the shared HID
    // stream, so an allocation/source error cannot leave part of a chord down.
    let events = plan.iter().map(&mut prepare).collect::<Result<Vec<_>>>()?;
    for event in events {
        emit(event);
    }
    Ok(())
}

pub fn key_chord(chord: &str) -> Result<()> {
    let (flags, code) = parse_chord(chord)?;
    run_chord_transaction(|| {
        let src = source()?;
        let baseline = CGEvent::new(src.clone())
            .map_err(|_| anyhow::anyhow!("failed to read keyboard modifier state"))?
            .get_flags();
        let plan = chord_event_plan(flags, code, baseline);
        prepare_and_post_chord(
            &plan,
            |event| {
                let cg_event = CGEvent::new_keyboard_event(src.clone(), event.code, event.down)
                    .map_err(|_| anyhow::anyhow!("failed to create chord keyboard event"))?;
                if event.modifier {
                    cg_event.set_type(CGEventType::FlagsChanged);
                }
                cg_event.set_flags(event.flags);
                Ok(cg_event)
            },
            |event| {
                post(event);
                sleep(Duration::from_millis(15));
            },
        )
    })
}

/// Parse a chord into (modifier flags, optional main keycode). Unlike
/// `parse_chord`, a modifier-only chord (e.g. "shift") is allowed and returns
/// `None` for the keycode. Used for key_down/key_up holds.
pub fn parse_chord_opt(chord: &str) -> Result<(CGEventFlags, Option<u16>)> {
    let mut flags = CGEventFlags::CGEventFlagNull;
    let mut keycode: Option<u16> = None;
    for raw in chord.split('+') {
        let part = raw.trim().to_lowercase();
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "cmd" | "command" | "meta" | "super" => flags |= CGEventFlags::CGEventFlagCommand,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            "alt" | "opt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "fn" => flags |= CGEventFlags::CGEventFlagSecondaryFn,
            other => {
                if keycode.is_some() {
                    bail!("key chord '{chord}' has more than one non-modifier key");
                }
                keycode = Some(
                    keys::keycode_for(other)
                        .with_context(|| format!("unknown key '{other}' in chord '{chord}'"))?,
                );
            }
        }
    }
    if keycode.is_none() && flags == CGEventFlags::CGEventFlagNull {
        bail!("chord '{chord}' is empty");
    }
    Ok((flags, keycode))
}

/// Keycode for a single modifier name, for modifier-only holds.
fn modifier_keycode(chord: &str) -> Option<u16> {
    match chord.trim().to_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" => Some(0x37),
        "shift" => Some(0x38),
        "alt" | "opt" | "option" => Some(0x3A),
        "ctrl" | "control" => Some(0x3B),
        "fn" => Some(0x3F),
        _ => None,
    }
}

/// Hold a key or chord down (down_state=true) or release it (false).
/// Supports modifier-only holds (e.g. "shift") via a FlagsChanged event.
pub fn key_hold(chord: &str, down_state: bool) -> Result<()> {
    let (flags, keycode) = parse_chord_opt(chord)?;
    let src = source()?;
    match keycode {
        Some(code) => {
            let evt = CGEvent::new_keyboard_event(src, code, down_state)
                .map_err(|_| anyhow::anyhow!("failed to create key event"))?;
            evt.set_flags(flags);
            post(evt);
        }
        None => {
            // Modifier-only: emit a FlagsChanged event carrying the modifier
            // keycode, with the flags set while held and cleared on release.
            let code = modifier_keycode(chord)
                .with_context(|| format!("unsupported modifier-only hold '{chord}'"))?;
            let evt = CGEvent::new_keyboard_event(src, code, down_state)
                .map_err(|_| anyhow::anyhow!("failed to create modifier event"))?;
            evt.set_type(CGEventType::FlagsChanged);
            evt.set_flags(if down_state {
                flags
            } else {
                CGEventFlags::CGEventFlagNull
            });
            post(evt);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(chord: &str, baseline: CGEventFlags) -> Vec<ChordKeyEvent> {
        let (flags, code) = parse_chord(chord).unwrap();
        chord_event_plan(flags, code, baseline)
    }

    fn event(code: u16, down: bool, modifier: bool, flags: CGEventFlags) -> ChordKeyEvent {
        ChordKeyEvent {
            code,
            down,
            modifier,
            flags,
        }
    }

    #[test]
    fn chord_plan_ctrl_g_releases_only_its_control_modifier() {
        let ctrl = CGEventFlags::CGEventFlagControl;
        let none = CGEventFlags::CGEventFlagNull;
        assert_eq!(
            plan("ctrl+g", none),
            vec![
                event(0x3B, true, true, ctrl),
                event(0x05, true, false, ctrl),
                event(0x05, false, false, ctrl),
                event(0x3B, false, true, none),
            ]
        );
    }

    #[test]
    fn chord_plan_cmd_shift_a_releases_in_reverse_order() {
        let cmd = CGEventFlags::CGEventFlagCommand;
        let both = cmd | CGEventFlags::CGEventFlagShift;
        let none = CGEventFlags::CGEventFlagNull;
        assert_eq!(
            plan("cmd+shift+a", none),
            vec![
                event(0x37, true, true, cmd),
                event(0x38, true, true, both),
                event(0x00, true, false, both),
                event(0x00, false, false, both),
                event(0x38, false, true, cmd),
                event(0x37, false, true, none),
            ]
        );
    }

    #[test]
    fn chord_plan_bare_key_does_not_inject_modifier_events() {
        let baseline = CGEventFlags::CGEventFlagShift | CGEventFlags::CGEventFlagAlphaShift;
        for flags in [CGEventFlags::CGEventFlagNull, baseline] {
            assert_eq!(
                plan("g", flags),
                vec![
                    event(0x05, true, false, flags),
                    event(0x05, false, false, flags),
                ]
            );
        }
    }

    #[test]
    fn chord_plan_preserves_unrelated_held_modifiers() {
        let baseline = CGEventFlags::CGEventFlagShift | CGEventFlags::CGEventFlagAlphaShift;
        let active = baseline | CGEventFlags::CGEventFlagControl;
        assert_eq!(
            plan("ctrl+g", baseline),
            vec![
                event(0x3B, true, true, active),
                event(0x05, true, false, active),
                event(0x05, false, false, active),
                event(0x3B, false, true, baseline),
            ]
        );
    }

    #[test]
    fn chord_plan_does_not_release_an_already_held_requested_modifier() {
        let baseline = CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagShift;
        assert_eq!(
            plan("ctrl+g", baseline),
            vec![
                event(0x05, true, false, baseline),
                event(0x05, false, false, baseline),
            ]
        );
    }

    #[test]
    fn chord_plan_fn_and_option_restore_unrelated_baseline_flags() {
        let baseline = CGEventFlags::CGEventFlagAlphaShift | CGEventFlags::CGEventFlagCommand;
        let alt = baseline | CGEventFlags::CGEventFlagAlternate;
        let active = alt | CGEventFlags::CGEventFlagSecondaryFn;
        assert_eq!(
            plan("fn+alt+left", baseline),
            vec![
                event(0x3A, true, true, alt),
                event(0x3F, true, true, active),
                event(0x7B, true, false, active),
                event(0x7B, false, false, active),
                event(0x3F, false, true, alt),
                event(0x3A, false, true, baseline),
            ]
        );
    }

    #[test]
    fn chord_preparation_failure_at_any_event_posts_nothing() {
        let plan = plan("cmd+shift+a", CGEventFlags::CGEventFlagNull);
        for failure_index in 0..plan.len() {
            let mut next_index = 0;
            let mut posted = Vec::new();
            let result = prepare_and_post_chord(
                &plan,
                |event| {
                    let index = next_index;
                    next_index += 1;
                    if index == failure_index {
                        bail!("simulated CGEvent creation failure");
                    }
                    Ok(*event)
                },
                |event| posted.push(event),
            );
            assert!(result.is_err());
            assert!(posted.is_empty(), "partial chord at event {failure_index}");
        }
    }

    #[test]
    fn chord_preparation_success_posts_the_entire_plan_in_order() {
        let plan = plan("ctrl+g", CGEventFlags::CGEventFlagNull);
        let mut posted = Vec::new();
        prepare_and_post_chord(&plan, |event| Ok(*event), |event| posted.push(event)).unwrap();
        assert_eq!(posted, plan);
    }

    #[test]
    fn chord_transactions_do_not_adopt_each_others_modifiers() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Barrier};

        let baseline = CGEventFlags::CGEventFlagAlphaShift;
        let fake_flags = Arc::new(AtomicU64::new(baseline.bits()));
        let start = Arc::new(Barrier::new(3));
        let workers: Vec<_> = ["ctrl+g", "cmd+shift+a"]
            .into_iter()
            .map(|chord| {
                let fake_flags = Arc::clone(&fake_flags);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..32 {
                        run_chord_transaction(|| {
                            let before = fake_flags.load(Ordering::SeqCst);
                            assert_eq!(before, baseline.bits());
                            let plan = plan(chord, CGEventFlags::from_bits_truncate(before));
                            prepare_and_post_chord(
                                &plan,
                                |event| Ok(*event),
                                |event| {
                                    fake_flags.store(event.flags.bits(), Ordering::SeqCst);
                                    std::thread::yield_now();
                                },
                            )?;
                            assert_eq!(fake_flags.load(Ordering::SeqCst), baseline.bits());
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        start.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(fake_flags.load(Ordering::SeqCst), baseline.bits());
    }

    #[test]
    fn parses_modifier_only_chord() {
        let (flags, code) = parse_chord_opt("shift").unwrap();
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(code.is_none());
    }

    #[test]
    fn parses_chord_with_key() {
        let (flags, code) = parse_chord_opt("cmd+a").unwrap();
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert_eq!(code, Some(0x00));
    }

    #[test]
    fn rejects_empty_chord() {
        assert!(parse_chord_opt("").is_err());
    }

    #[test]
    fn modifier_keycodes_known() {
        assert_eq!(modifier_keycode("cmd"), Some(0x37));
        assert_eq!(modifier_keycode("shift"), Some(0x38));
        assert_eq!(modifier_keycode("nope"), None);
    }
}
