// Independent translation of Rust std 1.98.1 Windows environment ordering/serialization.
// Rust is licensed MIT OR Apache-2.0: https://github.com/rust-lang/rust/blob/master/LICENSE-MIT
// https://github.com/rust-lang/rust/blob/master/LICENSE-APACHE
use std::{cmp::Ordering, collections::BTreeMap, ffi::OsString, io, os::windows::ffi::OsStrExt};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(a: *const u16, alen: i32, b: *const u16, blen: i32, ignore: i32)
    -> i32;
}
#[derive(Eq)]
struct Key(Vec<u16>);
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        let a = i32::try_from(self.0.len()).expect("environment key too long");
        let b = i32::try_from(other.0.len()).expect("environment key too long");
        match unsafe { CompareStringOrdinal(self.0.as_ptr(), a, other.0.as_ptr(), b, 1) } {
            1 => Ordering::Less,
            2 => Ordering::Equal,
            3 => Ordering::Greater,
            _ => panic!(
                "CompareStringOrdinal failed: {}",
                io::Error::last_os_error()
            ),
        }
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
pub(super) fn same_key(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    Key(a.encode_wide().collect()) == Key(b.encode_wide().collect())
}
pub(super) fn environment_block(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
) -> io::Result<Vec<u16>> {
    let mut map = BTreeMap::new();
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
        map.insert(Key(k), v);
    }
    let mut block = Vec::new();
    for (key, value) in map {
        block.extend(key.0);
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
