use chrono::Utc;

pub fn new_id(prefix: &str) -> String {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();
    format!("{}_{}_{}", prefix, ts, rand)
}

/// Server modifiers (adjectives/verbs) with their icons
/// Servers use modifiers while sessions use nouns, so together they form phrases like "blazing fox"
const SERVER_MODIFIERS: &[(&str, &str)] = &[
    // Adjectives
    ("blazing", "🔥"),
    ("frozen", "❄️"),
    ("swift", "⚡"),
    ("dark", "🌑"),
    ("bright", "✨"),
    ("crystal", "💎"),
    ("iron", "⚙️"),
    ("wild", "🌿"),
    ("stone", "🪨"),
    ("silent", "🔇"),
    ("golden", "⭐"),
    ("ancient", "🏛️"),
    ("stormy", "⛈️"),
    ("misty", "🌫️"),
    ("icy", "🧊"),
    ("cosmic", "🌌"),
    ("lunar", "🌙"),
    ("solar", "☀️"),
    ("crimson", "🔴"),
    ("azure", "🔵"),
    ("emerald", "💚"),
    ("amber", "🟠"),
    ("violet", "🟣"),
    ("proud", "👑"),
    ("hollow", "🕳️"),
    // Verbs (present participle)
    ("rising", "🌅"),
    ("falling", "🍂"),
    ("rushing", "🌊"),
    ("spinning", "💫"),
    ("blooming", "🌸"),
    ("sleeping", "💤"),
    ("flowing", "💧"),
    ("drifting", "🍃"),
    ("howling", "🌬️"),
    ("dancing", "💃"),
    ("dreaming", "💭"),
    ("seeking", "🔍"),
    ("waiting", "⏳"),
    ("burning", "🔥"),
    ("glowing", "✨"),
];

/// Session names with their icons - only words with specific emojis
const SESSION_NAMES: &[(&str, &str)] = &[
    // Animals
    ("ant", "🐜"),
    ("bat", "🦇"),
    ("bee", "🐝"),
    ("cat", "🐱"),
    ("cow", "🐄"),
    ("dog", "🐕"),
    ("fox", "🦊"),
    ("owl", "🦉"),
    ("pig", "🐷"),
    ("rat", "🐀"),
    ("bear", "🐻"),
    ("bird", "🐦"),
    ("crab", "🦀"),
    ("crow", "🐦‍⬛"),
    ("deer", "🦌"),
    ("dove", "🕊️"),
    ("duck", "🦆"),
    ("frog", "🐸"),
    ("goat", "🐐"),
    ("hawk", "🦅"),
    ("lion", "🦁"),
    ("moth", "🦋"),
    ("swan", "🦢"),
    ("wolf", "🐺"),
    ("zebra", "🦓"),
    ("eagle", "🦅"),
    ("goose", "🪿"),
    ("horse", "🐴"),
    ("koala", "🐨"),
    ("llama", "🦙"),
    ("moose", "🫎"),
    ("mouse", "🐭"),
    ("otter", "🦦"),
    ("panda", "🐼"),
    ("raven", "🐦‍⬛"),
    ("shark", "🦈"),
    ("sheep", "🐑"),
    ("sloth", "🦥"),
    ("snail", "🐌"),
    ("snake", "🐍"),
    ("squid", "🦑"),
    ("tiger", "🐯"),
    ("whale", "🐋"),
    ("turtle", "🐢"),
    ("rabbit", "🐰"),
    ("parrot", "🦜"),
    ("falcon", "🦅"),
    ("jaguar", "🐆"),
    ("lizard", "🦎"),
    // Nature
    ("sun", "☀️"),
    ("moon", "🌙"),
    ("star", "⭐"),
    ("fire", "🔥"),
    ("snow", "❄️"),
    ("rain", "🌧️"),
    ("wind", "💨"),
    ("wave", "🌊"),
    ("leaf", "🍃"),
    ("tree", "🌲"),
    ("rose", "🌹"),
    ("pine", "🌲"),
    ("oak", "🌳"),
    ("fern", "🌿"),
    ("moss", "🌱"),
    ("cloud", "☁️"),
    ("storm", "⛈️"),
    ("frost", "🥶"),
    ("coral", "🪸"),
    ("gem", "💎"),
    ("jade", "💚"),
    ("pearl", "🦪"),
    ("amber", "🟠"),
    ("lake", "🏞️"),
    ("river", "🏞️"),
    ("creek", "💧"),
    ("brook", "💧"),
    ("rock", "🪨"),
    ("stone", "🪨"),
    ("cliff", "🏔️"),
    ("peak", "⛰️"),
    ("summit", "🏔️"),
    ("meadow", "🌾"),
    ("grove", "🌳"),
    ("marsh", "🌿"),
];

/// Get an emoji icon for a session name word
pub fn session_icon(name: &str) -> &'static str {
    SESSION_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, icon)| *icon)
        .unwrap_or("💫")
}

/// Get an emoji icon for a server modifier
pub fn server_icon(name: &str) -> &'static str {
    SERVER_MODIFIERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, icon)| *icon)
        .unwrap_or("🔮")
}

/// Generate a memorable server name
/// Returns (full_id, short_name) where:
/// - full_id is the storage identifier like "server_blazing_1234567890"
/// - short_name is the memorable part like "blazing"
pub fn new_memorable_server_id() -> (String, String) {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();

    // Use the random value to pick a modifier
    let idx = (rand as usize) % SERVER_MODIFIERS.len();
    let (word, _) = SERVER_MODIFIERS[idx];

    let short_name = word.to_string();
    let full_id = format!("server_{}_{}", word, ts);

    (full_id, short_name)
}

/// Try to extract the memorable name from a server ID
/// e.g., "server_blazing_1234567890" -> Some("blazing")
pub fn extract_server_name(server_id: &str) -> Option<&str> {
    if server_id.starts_with("server_") {
        let rest = &server_id[7..]; // Skip "server_"
        if let Some(pos) = rest.rfind('_') {
            return Some(&rest[..pos]);
        }
    }
    None
}

/// Generate a memorable session name
/// Returns (full_id, short_name) where:
/// - full_id is the storage identifier like "session_fox_1234567890"
/// - short_name is the memorable part like "fox"
pub fn new_memorable_session_id() -> (String, String) {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();

    // Use the random value to pick a word
    let idx = (rand as usize) % SESSION_NAMES.len();
    let (word, _) = SESSION_NAMES[idx];

    let short_name = word.to_string();
    let full_id = format!("session_{}_{}", word, ts);

    (full_id, short_name)
}

/// Try to extract the memorable name from a session ID
/// e.g., "session_fox_1234567890" -> Some("fox")
pub fn extract_session_name(session_id: &str) -> Option<&str> {
    if session_id.starts_with("session_") {
        let rest = &session_id[8..]; // Skip "session_"
                                     // Find the last underscore (before timestamp)
        if let Some(pos) = rest.rfind('_') {
            return Some(&rest[..pos]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_memorable_session_id() {
        let (full_id, short_name) = new_memorable_session_id();

        // Full ID should start with "session_"
        assert!(full_id.starts_with("session_"));

        // Short name should be non-empty
        assert!(!short_name.is_empty());

        // Full ID should contain the short name
        assert!(full_id.contains(&short_name));

        // Short name should have a specific icon (not default)
        let icon = session_icon(&short_name);
        assert_ne!(
            icon, "💫",
            "Name '{}' should have a specific icon",
            short_name
        );
    }

    #[test]
    fn test_extract_session_name() {
        assert_eq!(extract_session_name("session_fox_1234567890"), Some("fox"));
        assert_eq!(
            extract_session_name("session_blue-whale_1234567890"),
            Some("blue-whale")
        );
        assert_eq!(
            extract_session_name("session_1234567890_9876543210"),
            Some("1234567890")
        );
        assert_eq!(extract_session_name("invalid"), None);
        assert_eq!(extract_session_name("session_"), None);
    }

    #[test]
    fn test_unique_session_ids() {
        let (id1, _) = new_memorable_session_id();
        // Small sleep to ensure timestamp differs
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (id2, _) = new_memorable_session_id();

        // Even with same word, timestamps should differ
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_all_names_have_icons() {
        for (name, expected_icon) in SESSION_NAMES {
            let icon = session_icon(name);
            assert_eq!(icon, *expected_icon, "Icon mismatch for '{}'", name);
            assert_ne!(icon, "💫", "Name '{}' should have a specific icon", name);
        }
    }

    #[test]
    fn test_new_memorable_server_id() {
        let (full_id, short_name) = new_memorable_server_id();

        // Full ID should start with "server_"
        assert!(full_id.starts_with("server_"));

        // Short name should be non-empty
        assert!(!short_name.is_empty());

        // Full ID should contain the short name
        assert!(full_id.contains(&short_name));

        // Short name should have a specific icon (not default)
        let icon = server_icon(&short_name);
        assert_ne!(
            icon, "🔮",
            "Modifier '{}' should have a specific icon",
            short_name
        );
    }

    #[test]
    fn test_extract_server_name() {
        assert_eq!(
            extract_server_name("server_blazing_1234567890"),
            Some("blazing")
        );
        assert_eq!(
            extract_server_name("server_rising_1234567890"),
            Some("rising")
        );
        assert_eq!(extract_server_name("invalid"), None);
        assert_eq!(extract_server_name("server_"), None);
    }

    #[test]
    fn test_all_modifiers_have_icons() {
        for (name, expected_icon) in SERVER_MODIFIERS {
            let icon = server_icon(name);
            assert_eq!(icon, *expected_icon, "Icon mismatch for '{}'", name);
            assert_ne!(
                icon, "🔮",
                "Modifier '{}' should have a specific icon",
                name
            );
        }
    }
}
