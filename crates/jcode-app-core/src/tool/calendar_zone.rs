//! IANA zone resolution from the bundled chrono-tz database, so local calendar
//! times resolve the same on every OS (Windows ships no zoneinfo files). No
//! process-global TZ changes: concurrent tools must not affect each other.
use anyhow::{Result, bail};
use chrono::{DateTime, FixedOffset, LocalResult, NaiveDateTime, TimeZone};

pub(super) fn local_in_zone(local: &str, zone: &str) -> Result<DateTime<FixedOffset>> {
    let naive = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S")?;
    let Ok(tz) = zone.parse::<chrono_tz::Tz>() else {
        bail!("Invalid IANA time zone {zone}; pass an RFC 3339 start with an explicit offset");
    };
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(time) => Ok(time.fixed_offset()),
        LocalResult::None => bail!(
            "Local start is nonexistent at a DST transition; pass RFC 3339 with an explicit offset"
        ),
        LocalResult::Ambiguous(..) => bail!(
            "Local start is ambiguous at a DST transition; pass RFC 3339 with an explicit offset"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::local_in_zone;

    #[test]
    fn resolves_zones_without_os_zoneinfo_and_rejects_dst_gaps_and_folds() {
        let winter = local_in_zone("2026-12-01T10:00:00", "America/New_York").unwrap();
        assert_eq!(winter.to_rfc3339(), "2026-12-01T10:00:00-05:00");
        let summer = local_in_zone("2026-07-01T10:00:00", "America/New_York").unwrap();
        assert_eq!(summer.to_rfc3339(), "2026-07-01T10:00:00-04:00");
        assert_eq!(
            local_in_zone("2026-07-01T10:00:00", "UTC")
                .unwrap()
                .to_rfc3339(),
            "2026-07-01T10:00:00+00:00"
        );
        // 2026-03-08 02:30 does not exist in New York; 2026-11-01 01:30 occurs twice.
        assert!(local_in_zone("2026-03-08T02:30:00", "America/New_York").is_err());
        assert!(local_in_zone("2026-11-01T01:30:00", "America/New_York").is_err());
        assert!(local_in_zone("2026-07-01T10:00:00", "Not/AZone").is_err());
    }
}
