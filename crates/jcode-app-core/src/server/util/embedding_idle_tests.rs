use super::*;

#[test]
fn idle_unload_defaults_to_one_minute_and_accepts_positive_override() {
    assert_eq!(parse_embedding_idle_unload_secs(None), 60);
    assert_eq!(parse_embedding_idle_unload_secs(Some("15")), 15);
    assert_eq!(parse_embedding_idle_unload_secs(Some("0")), 60);
    assert_eq!(parse_embedding_idle_unload_secs(Some("invalid")), 60);
}
