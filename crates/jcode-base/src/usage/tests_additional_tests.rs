use super::*;

#[test]
fn test_usage_data_default() {
    let data = UsageData::default();
    assert!(data.is_stale());
    assert_eq!(data.five_hour_percent(), "0%");
    assert_eq!(data.seven_day_percent(), "0%");
}

#[test]
fn test_usage_percent_format() {
    let data = UsageData {
        five_hour: 0.42,
        seven_day: 0.156,
        ..Default::default()
    };
    assert_eq!(data.five_hour_percent(), "42%");
    assert_eq!(data.seven_day_percent(), "16%");
}

#[test]
fn test_humanize_key() {
    assert_eq!(humanize_key("five_hour"), "Five Hour");
    assert_eq!(humanize_key("seven_day_opus"), "Seven Day Opus");
    assert_eq!(humanize_key("plan"), "Plan");
}
