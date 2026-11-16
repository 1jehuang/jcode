//! Names and locations describe the session itself, unlike transcript mentions.
use super::{SessionInfo, transcript_search::lowercase_field};

/// Complete structured matches lead. An exact animal/name and actual cloud
/// ownership break ties, so `swan cloud` finds the cloud swan even if renamed.
/// Partial identity matches must not elevate unrelated transcript results.
pub(super) fn rank(session: &SessionInfo, tokens: &[String]) -> (bool, bool, bool) {
    if tokens.is_empty() {
        return (false, false, false);
    }
    let cloud = session.server_icon.as_deref() == Some("☁");
    let fields = [
        session.id.as_str(),
        session.short_name.as_str(),
        session.title.as_str(),
        session.save_label.as_deref().unwrap_or(""),
        session.working_dir.as_deref().unwrap_or(""),
        session.server_name.as_deref().unwrap_or(""),
    ]
    .map(lowercase_field);
    let complete = tokens.iter().all(|token| {
        (cloud && token == "cloud") || fields.iter().any(|field| field.contains(token.as_str()))
    });
    if !complete {
        return (false, false, false);
    }
    let name = lowercase_field(&session.short_name);
    let exact_name = tokens.iter().any(|token| token == name.as_ref());
    let cloud_location = cloud && tokens.iter().any(|token| token == "cloud");
    (true, exact_name, cloud_location)
}
