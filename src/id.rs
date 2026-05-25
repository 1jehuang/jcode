use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn extract_session_name(id: &str) -> Option<String> {
    Some(id.to_string())
}

pub fn new_memorable_session_id() -> String {
    format!("session-{}", Uuid::new_v4().simple()[..8].to_lowercase())
}

pub fn new_memorable_server_id() -> (String, String) {
    (Uuid::new_v4().to_string(), "Server".into())
}

pub fn server_icon() -> &'static str {
    "🖥️"
}

pub fn session_icon(_name: &str) -> &'static str {
    "💬"
}
