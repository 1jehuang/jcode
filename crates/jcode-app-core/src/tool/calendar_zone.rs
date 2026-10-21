//! Small, read-only adapter for the OS IANA zone database. No process-global TZ
//! changes: concurrent tools must not affect each other's time interpretation.
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};

pub(super) fn local_in_zone(local: &str, zone: &str) -> Result<DateTime<FixedOffset>> {
    let naive = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S")?;
    if matches!(zone, "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT") {
        return Ok(naive.and_utc().fixed_offset());
    }
    ensure!(
        !zone.starts_with('/')
            && zone
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && zone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b)),
        "Invalid IANA time zone; pass an RFC 3339 start with an explicit offset"
    );
    let mut data = None;
    for base in ["/usr/share/zoneinfo", "/var/db/timezone/zoneinfo"] {
        match std::fs::read(format!("{base}/{zone}")) {
            Ok(bytes) => {
                data = Some(bytes);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).context("Could not read the installed time-zone database");
            }
        }
    }
    let data = data.with_context(|| {
        format!("Time zone {zone} unavailable; pass an RFC 3339 start with an explicit offset")
    })?;
    resolve(&data, naive)
}

fn count(data: &[u8], at: usize) -> Result<usize> {
    let bytes: [u8; 4] = data
        .get(at..at + 4)
        .context("Truncated TZif header")?
        .try_into()?;
    Ok(u32::from_be_bytes(bytes) as usize)
}

fn block_len(data: &[u8], at: usize, width: usize) -> Result<usize> {
    let counts = (0..6)
        .map(|n| count(data, at + 20 + n * 4))
        .collect::<Result<Vec<_>>>()?;
    Ok(counts[3] * (width + 1)
        + counts[4] * 6
        + counts[5]
        + counts[2] * (width + 4)
        + counts[0]
        + counts[1])
}

fn resolve(data: &[u8], naive: NaiveDateTime) -> Result<DateTime<FixedOffset>> {
    ensure!(data.get(..4) == Some(b"TZif"), "Invalid TZif database");
    let (header, width) = if matches!(data.get(4), Some(b'2' | b'3' | b'4')) {
        (44 + block_len(data, 0, 4)?, 8)
    } else {
        (0, 4)
    };
    let transitions = count(data, header + 32)?;
    let types = count(data, header + 36)?;
    let start = header + 44;
    let indexes = start + transitions * width;
    let type_start = indexes + transitions;
    let end = start + block_len(data, header, width)?;
    ensure!(end <= data.len() && types > 0, "Truncated TZif database");
    let offsets = (0..types)
        .map(|n| {
            let at = type_start + n * 6;
            let bytes: [u8; 4] = data[at..at + 4].try_into()?;
            Ok(i32::from_be_bytes(bytes))
        })
        .collect::<Result<Vec<_>>>()?;
    let times = (0..transitions)
        .map(|n| {
            let at = start + n * width;
            Ok(if width == 8 {
                i64::from_be_bytes(data[at..at + 8].try_into()?)
            } else {
                i32::from_be_bytes(data[at..at + 4].try_into()?) as i64
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut matches = Vec::new();
    for offset in &offsets {
        let Some(offset) = FixedOffset::east_opt(*offset) else {
            continue;
        };
        let Some(candidate) = offset.from_local_datetime(&naive).single() else {
            continue;
        };
        let instant = candidate.timestamp();
        let next = times.partition_point(|time| *time <= instant);
        // TZif's POSIX tail can extend seasonal rules beyond the explicit
        // table. Fail closed there rather than silently guessing a DST offset.
        if next == times.len()
            && !times.is_empty()
            && instant > times[times.len() - 1]
            && data
                .get(end..)
                .is_some_and(|tail| tail.iter().any(|b| *b != b'\n' && *b != 0))
        {
            continue;
        }
        let kind = if next == 0 {
            0
        } else {
            data[indexes + next - 1] as usize
        };
        ensure!(kind < types, "Invalid TZif transition type");
        if offsets[kind] == offset.local_minus_utc() && !matches.contains(&candidate) {
            matches.push(candidate);
        }
    }
    match matches.as_slice() {
        [only] => Ok(*only),
        [] => bail!(
            "Local start is nonexistent or outside the installed time-zone table; pass RFC 3339 with an explicit offset"
        ),
        _ => bail!(
            "Local start is ambiguous at a DST transition; pass RFC 3339 with an explicit offset"
        ),
    }
}
