use super::*;

#[test]
fn upload_http_failures_have_fixed_actionable_labels() {
    for (code, expected) in [
        (400, "http_error"),
        (401, "unauthorized"),
        (403, "forbidden"),
        (404, "route_missing"),
        (413, "payload_too_large"),
        (429, "rate_limited"),
        (500, "server_error"),
        (503, "server_error"),
        (599, "server_error"),
    ] {
        let outcome = upload_http_outcome(reqwest::StatusCode::from_u16(code).unwrap());
        assert_eq!(outcome, expected);
        let mut timing = SyncTiming::new("http_upload");
        timing.outcome = outcome;
        let record = timing.record();
        assert_eq!(record.split_whitespace().count(), 4);
        assert!(record.contains(&format!("outcome={expected}")));
    }
}
