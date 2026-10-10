//! Korean 2-Set keyboard layout handling for modified terminal key events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Fold a modified Korean jamo to the Latin letter on the same physical key.
/// Unmodified Korean input remains unchanged.
pub fn normalize_key_event(mut key: KeyEvent) -> KeyEvent {
    let chord = KeyModifiers::CONTROL
        | KeyModifiers::ALT
        | KeyModifiers::SUPER
        | KeyModifiers::META
        | KeyModifiers::HYPER;
    if key.modifiers.intersects(chord)
        && let KeyCode::Char(c) = key.code
        && let Some(latin) = dubeolsik_jamo_to_latin(c)
    {
        key.code = KeyCode::Char(latin);
    }
    key
}

/// Map a Korean 2-Set jamo to the US-QWERTY letter on the same physical key.
fn dubeolsik_jamo_to_latin(ch: char) -> Option<char> {
    Some(match ch {
        'ㅂ' => 'q',
        'ㅈ' => 'w',
        'ㄷ' => 'e',
        'ㄱ' => 'r',
        'ㅅ' => 't',
        'ㅛ' => 'y',
        'ㅕ' => 'u',
        'ㅑ' => 'i',
        'ㅐ' => 'o',
        'ㅔ' => 'p',
        'ㅁ' => 'a',
        'ㄴ' => 's',
        'ㅇ' => 'd',
        'ㄹ' => 'f',
        'ㅎ' => 'g',
        'ㅗ' => 'h',
        'ㅓ' => 'j',
        'ㅏ' => 'k',
        'ㅣ' => 'l',
        'ㅋ' => 'z',
        'ㅌ' => 'x',
        'ㅊ' => 'c',
        'ㅍ' => 'v',
        'ㅠ' => 'b',
        'ㅜ' => 'n',
        'ㅡ' => 'm',
        'ㅃ' => 'Q',
        'ㅉ' => 'W',
        'ㄸ' => 'E',
        'ㄲ' => 'R',
        'ㅆ' => 'T',
        'ㅒ' => 'O',
        'ㅖ' => 'P',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dubeolsik_jamo_map_to_their_physical_latin_keys() {
        let row = "ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔㅁㄴㅇㄹㅎㅗㅓㅏㅣㅋㅌㅊㅍㅠㅜㅡ";
        let latin: String = row.chars().filter_map(dubeolsik_jamo_to_latin).collect();
        assert_eq!(latin, "qwertyuiopasdfghjklzxcvbnm");
        assert_eq!(dubeolsik_jamo_to_latin('ㅃ'), Some('Q'));
        assert_eq!(dubeolsik_jamo_to_latin('가'), None);
        assert_eq!(dubeolsik_jamo_to_latin('v'), None);
    }
}
