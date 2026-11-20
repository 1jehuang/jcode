//! Keep Ctrl+letter shortcuts working under non-Latin keyboard layouts.
//!
//! On Windows crossterm resolves a Ctrl+letter key event through the active
//! layout (`ToUnicodeEx`), and its `KeyEvent` carries neither the virtual-key
//! code nor the scan code. Under a Russian layout Ctrl+C therefore arrives as
//! `Char('с')` (U+0441) and never matches `Char('c')` (issues #2212, #2214).
//! Since the physical key is not available, map the Cyrillic letter back to
//! the Latin letter on the same key of the standard ЙЦУКЕН layout.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

fn latin_for_cyrillic(ch: char) -> Option<char> {
    let lower = match ch {
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'ф' => 'a',
        'ы' | 'і' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        _ => return None,
    };
    Some(lower)
}

/// Rewrite a Ctrl+Cyrillic-letter key press to the Latin letter on the same
/// physical key. Events with Alt, without Ctrl, or with any other character
/// pass through unchanged, so plain typing and Alt+AltGr input are untouched.
pub(super) fn normalize_ctrl_layout(event: Event) -> Event {
    let Event::Key(key) = event else {
        return event;
    };
    if !key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
        return Event::Key(key);
    }
    let KeyCode::Char(ch) = key.code else {
        return Event::Key(key);
    };
    let mut lowered = ch.to_lowercase();
    let (Some(single), None) = (lowered.next(), lowered.next()) else {
        return Event::Key(key);
    };
    let Some(latin) = latin_for_cyrillic(single) else {
        return Event::Key(key);
    };
    let latin = if ch.is_uppercase() {
        latin.to_ascii_uppercase()
    } else {
        latin
    };
    Event::Key(KeyEvent {
        code: KeyCode::Char(latin),
        ..key
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(ch: char, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(ch), modifiers))
    }

    #[test]
    fn russian_ctrl_c_becomes_latin_ctrl_c() {
        // U+0441 is what crossterm reports for the C key on a Russian layout.
        assert_eq!(
            normalize_ctrl_layout(key('\u{0441}', KeyModifiers::CONTROL)),
            key('c', KeyModifiers::CONTROL)
        );
    }

    #[test]
    fn every_mapped_letter_lands_on_its_qwerty_key() {
        let pairs = "йq цw уe кr еt нy гu шi щo зp фa ыs вd аf пg рh оj лk дl яz чx сc мv иb тn ьm";
        for pair in pairs.split(' ') {
            let mut chars = pair.chars();
            let (cyr, lat) = (chars.next().unwrap(), chars.next().unwrap());
            assert_eq!(
                normalize_ctrl_layout(key(cyr, KeyModifiers::CONTROL)),
                key(lat, KeyModifiers::CONTROL),
                "{cyr}"
            );
        }
    }

    #[test]
    fn shifted_letter_keeps_case_and_modifiers() {
        let mods = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(normalize_ctrl_layout(key('\u{0421}', mods)), key('C', mods));
    }

    #[test]
    fn ukrainian_i_maps_to_s() {
        assert_eq!(
            normalize_ctrl_layout(key('\u{0456}', KeyModifiers::CONTROL)),
            key('s', KeyModifiers::CONTROL)
        );
    }

    #[test]
    fn other_events_are_untouched() {
        // Plain typing in Cyrillic.
        let plain = key('\u{0441}', KeyModifiers::NONE);
        assert_eq!(normalize_ctrl_layout(plain.clone()), plain);
        // Ctrl+Alt is AltGr territory on some layouts.
        let altgr = key('\u{0441}', KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(normalize_ctrl_layout(altgr.clone()), altgr);
        // Latin Ctrl+C and non-letter Cyrillic stay as they are.
        let latin = key('c', KeyModifiers::CONTROL);
        assert_eq!(normalize_ctrl_layout(latin.clone()), latin);
        let yo = key('\u{0451}', KeyModifiers::CONTROL);
        assert_eq!(normalize_ctrl_layout(yo.clone()), yo);
        assert_eq!(
            normalize_ctrl_layout(Event::FocusGained),
            Event::FocusGained
        );
    }

    #[test]
    fn release_kind_and_state_survive() {
        let event = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\u{0441}'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        ));
        let Event::Key(out) = normalize_ctrl_layout(event) else {
            unreachable!("key event stays a key event");
        };
        assert_eq!(out.code, KeyCode::Char('c'));
        assert_eq!(out.kind, KeyEventKind::Repeat);
    }
}
