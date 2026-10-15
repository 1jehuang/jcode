use super::*;

#[test]
fn revoked_account_reports_successful_cleanup() {
    let message = revoked_account_status_message(Ok(()));
    assert!(message.contains("Local credentials were cleared"));
    assert!(message.contains("/account jcode login"));
}

#[test]
fn revoked_account_does_not_claim_failed_cleanup_succeeded() {
    let message = revoked_account_status_message(Err(anyhow::anyhow!("permission denied")));
    assert!(message.contains("permission denied"));
    assert!(message.contains("could not be cleared"));
    assert!(message.contains("/account jcode logout"));
    assert!(!message.contains("Local credentials were cleared"));
}
