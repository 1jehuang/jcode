// Scratch translation of Rust std 1.98.1 batch encoder; MIT OR Apache-2.0.
// https://github.com/rust-lang/rust/blob/master/LICENSE-MIT
// https://github.com/rust-lang/rust/blob/master/LICENSE-APACHE
use std::os::windows::ffi::OsStrExt;
pub fn encode_batch_arg(arg: &std::ffi::OsStr) -> Result<Vec<u16>, &'static str> {
    let units: Vec<u16> = arg.encode_wide().collect();
    if units.iter().any(|c| matches!(*c, 0 | 10 | 13)) {
        return Err("invalid batch argument");
    }
    let safe = "#$*+-./:?@\\_";
    let quote = units.is_empty()
        || units.last() == Some(&92)
        || std::char::decode_utf16(units.iter().copied()).any(|c| match c {
            Ok(c) => {
                c.is_control() || (c.is_ascii() && !(c.is_ascii_alphanumeric() || safe.contains(c)))
            }
            Err(_) => false,
        });
    let mut out = Vec::new();
    if quote {
        out.push(34);
    }
    let mut slashes = 0;
    for c in units {
        if c == 92 {
            slashes += 1;
            out.push(c);
            continue;
        }
        if c == 34 {
            out.extend(std::iter::repeat_n(92, slashes));
            out.push(34);
        } else if c == 37 {
            out.extend("%%cd:~,".encode_utf16());
        }
        slashes = 0;
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n(92, slashes));
        out.push(34);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;
    #[test]
    fn preserves_surrogate_and_rejects_invalid() {
        let s = std::ffi::OsString::from_wide(&[0xd800]);
        assert_eq!(encode_batch_arg(&s).unwrap(), vec![0xd800]);
        for x in [0, 10, 13] {
            assert!(encode_batch_arg(&std::ffi::OsString::from_wide(&[x])).is_err());
        }
    }
}
use std::os::windows::ffi::OsStringExt;
pub(super) fn batch_line(
    script: &std::ffi::OsStr,
    args: &[std::ffi::OsString],
) -> std::io::Result<std::ffi::OsString> {
    let path: Vec<u16> = script.encode_wide().collect();
    if path.contains(&0) || path.contains(&34) || path.last() == Some(&92) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid batch path",
        ));
    }
    let mut line: Vec<u16> = "cmd.exe /e:ON /v:OFF /d /c \"\"".encode_utf16().collect();
    line.extend(path);
    line.push(34);
    for arg in args {
        line.push(32);
        line.extend(
            encode_batch_arg(arg)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
        );
    }
    line.push(34);
    Ok(std::ffi::OsString::from_wide(&line))
}
pub(super) fn exe_line(
    program: &std::ffi::OsStr,
    args: &[std::ffi::OsString],
) -> std::io::Result<std::ffi::OsString> {
    let mut line = vec![34];
    let argv0: Vec<u16> = program.encode_wide().collect();
    if argv0.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "NUL in program",
        ));
    }
    line.extend(argv0);
    line.push(34);
    for arg in args {
        let units: Vec<u16> = arg.encode_wide().collect();
        if units.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NUL in argument",
            ));
        }
        line.push(32);
        let quote = units.is_empty() || units.iter().any(|c| *c == 32 || *c == 9);
        if quote {
            line.push(34);
        }
        let mut slashes = 0;
        for c in units {
            if c == 92 {
                slashes += 1;
            } else {
                if c == 34 {
                    line.extend(std::iter::repeat_n(92, slashes + 1));
                }
                slashes = 0;
            }
            line.push(c);
        }
        if quote {
            line.extend(std::iter::repeat_n(92, slashes));
            line.push(34);
        }
    }
    Ok(std::ffi::OsString::from_wide(&line))
}
