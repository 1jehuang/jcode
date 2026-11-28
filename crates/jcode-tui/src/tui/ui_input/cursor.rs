/// Paste and remote byte offsets can land inside a multibyte character.
/// Clamp to its start before slicing input.
fn bounded_cursor(input: &str, cursor: usize) -> usize {
    let cursor = cursor.min(input.len());
    if input.is_char_boundary(cursor) {
        cursor
    } else {
        let mut idx = cursor;
        while idx > 0 && !input.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }
}
