// Windows environment ordering/serialization with Rust std 1.98.1 semantics.
// Comparison failures are returned before launching a child.
// Rust is licensed MIT OR Apache-2.0: https://github.com/rust-lang/rust/blob/master/LICENSE-MIT
// https://github.com/rust-lang/rust/blob/master/LICENSE-APACHE
use std::{cmp::Ordering, ffi::OsString, io, os::windows::ffi::OsStrExt};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(a: *const u16, alen: i32, b: *const u16, blen: i32, ignore: i32)
    -> i32;
}
fn compare_keys(a: &[u16], b: &[u16]) -> io::Result<Ordering> {
    let a_len = i32::try_from(a.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "environment key too long"))?;
    let b_len = i32::try_from(b.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "environment key too long"))?;
    match unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) } {
        1 => Ok(Ordering::Less),
        2 => Ok(Ordering::Equal),
        3 => Ok(Ordering::Greater),
        _ => Err(io::Error::last_os_error()),
    }
}
pub(super) fn same_key(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> io::Result<bool> {
    compare_keys(
        &a.encode_wide().collect::<Vec<_>>(),
        &b.encode_wide().collect::<Vec<_>>(),
    )
    .map(|ordering| ordering == Ordering::Equal)
}
pub(super) fn environment_block(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<Vec<u16>> {
    environment_block_using(entries, compare_keys)
}

fn environment_block_using(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
    mut compare: impl FnMut(&[u16], &[u16]) -> io::Result<Ordering>,
) -> io::Result<Vec<u16>> {
    let mut map: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
    for (key, value) in entries {
        let k: Vec<u16> = key.encode_wide().collect();
        let v: Vec<u16> = value.encode_wide().collect();
        if k.is_empty()
            || k.contains(&0)
            || k[1..].contains(&61)
            || v.contains(&0)
            || k.len() > i32::MAX as usize
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid environment entry",
            ));
        }
        let (mut left, mut right) = (0, map.len());
        let mut existing = None;
        while left < right {
            let middle = left + (right - left) / 2;
            match compare(&map[middle].0, &k)? {
                Ordering::Less => left = middle + 1,
                Ordering::Greater => right = middle,
                Ordering::Equal => {
                    existing = Some(middle);
                    break;
                }
            }
        }
        if let Some(index) = existing {
            map[index].1 = v;
        } else {
            map.insert(left, (k, v));
        }
    }
    let mut block = Vec::new();
    for (key, value) in map {
        block.extend(key);
        block.push(61);
        block.extend(value);
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}
pub(super) fn inherited_with_overrides(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<Vec<u16>> {
    environment_block(std::env::vars_os().chain(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_block_retains_first_key_casing_and_windows_order() {
        let entries = [
            ("Z", "tail"),
            ("Path", "one"),
            ("PATH", "two"),
            ("a", "first"),
        ]
        .map(|(key, value)| (key.into(), value.into()));
        let expected: Vec<u16> = "a=first\0Path=two\0Z=tail\0\0".encode_utf16().collect();
        assert_eq!(environment_block(entries).unwrap(), expected);
    }

    #[test]
    fn environment_block_propagates_comparison_errors() {
        let entries = [("A".into(), "one".into()), ("B".into(), "two".into())];
        let error = environment_block_using(entries, |_, _| {
            Err(io::Error::other("forced comparison failure"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "forced comparison failure");
    }

    #[test]
    fn environment_block_uses_unicode_ordinal_case_and_preserves_utf16_values() {
        use std::os::windows::ffi::OsStringExt;
        let entries = [
            ("\u{00e4}".into(), "old".into()),
            ("Z".into(), "ascii".into()),
            ("\u{00c4}".into(), OsString::from_wide(&[0xd800, 0x2603])),
        ];
        let mut expected: Vec<u16> = "Z=ascii\0\u{00e4}=".encode_utf16().collect();
        expected.extend([0xd800, 0x2603, 0, 0]);
        assert_eq!(environment_block(entries).unwrap(), expected);
    }
}
