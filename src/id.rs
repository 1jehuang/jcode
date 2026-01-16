use chrono::Utc;

pub fn new_id(prefix: &str) -> String {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();
    format!("{}_{}_{}", prefix, ts, rand)
}

/// Memorable words for session naming - easy to type and remember
const WORDS: &[&str] = &[
    // Animals
    "ant", "bat", "bee", "cat", "cow", "dog", "elk", "fox", "hen", "jay",
    "owl", "pig", "rat", "yak", "bear", "bird", "crab", "crow", "deer", "dove",
    "duck", "frog", "goat", "hawk", "lamb", "lion", "mole", "moth", "puma", "seal",
    "slug", "swan", "toad", "wasp", "wolf", "worm", "zebra", "crane", "eagle", "finch",
    "gecko", "goose", "heron", "horse", "koala", "lemur", "llama", "moose", "mouse", "otter",
    "panda", "raven", "shark", "sheep", "skunk", "sloth", "snail", "snake", "squid", "stork",
    "tiger", "trout", "viper", "whale", "badger", "beetle", "bobcat", "ferret", "falcon",
    "gopher", "jaguar", "lizard", "magpie", "mantis", "marmot", "osprey", "parrot", "pelican",
    "pigeon", "rabbit", "racoon", "salmon", "turtle", "walrus", "weasel",
    // Objects & Nature
    "ash", "bay", "elm", "fir", "gem", "ice", "ivy", "oak", "ore", "sun",
    "beam", "bolt", "clay", "coal", "cone", "cork", "dune", "fern", "fire", "foam",
    "glen", "hail", "jade", "lake", "leaf", "lime", "mist", "moon", "moss", "peak",
    "pine", "pond", "rain", "reed", "rock", "root", "rose", "sand", "snow", "star",
    "tide", "tree", "vale", "vine", "wave", "wind", "amber", "birch", "brook", "cedar",
    "cliff", "cloud", "coral", "creek", "delta", "flint", "frost", "grove", "maple", "marsh",
    "pearl", "plum", "ridge", "river", "shore", "slate", "steel", "stone", "storm", "thorn",
    "willow", "crystal", "meadow", "pebble", "summit",
];

/// Get an emoji icon for a session name word
pub fn session_icon(name: &str) -> &'static str {
    match name {
        // Animals with specific emojis
        "ant" => "🐜", "bat" => "🦇", "bee" => "🐝", "cat" => "🐱", "cow" => "🐄",
        "dog" => "🐕", "fox" => "🦊", "owl" => "🦉", "pig" => "🐷", "rat" => "🐀",
        "bear" => "🐻", "bird" => "🐦", "crab" => "🦀", "crow" => "🐦‍⬛", "deer" => "🦌",
        "dove" => "🕊️", "duck" => "🦆", "frog" => "🐸", "goat" => "🐐", "hawk" => "🦅",
        "lion" => "🦁", "moth" => "🦋", "swan" => "🦢", "wolf" => "🐺", "zebra" => "🦓",
        "eagle" => "🦅", "goose" => "🪿", "horse" => "🐴", "koala" => "🐨", "llama" => "🦙",
        "moose" => "🫎", "mouse" => "🐭", "otter" => "🦦", "panda" => "🐼", "raven" => "🐦‍⬛",
        "shark" => "🦈", "sheep" => "🐑", "sloth" => "🦥", "snail" => "🐌", "snake" => "🐍",
        "squid" => "🦑", "tiger" => "🐯", "whale" => "🐋", "turtle" => "🐢", "rabbit" => "🐰",
        "parrot" => "🦜", "falcon" => "🦅", "jaguar" => "🐆", "lizard" => "🦎",
        // Nature with specific emojis
        "sun" => "☀️", "moon" => "🌙", "star" => "⭐", "fire" => "🔥", "snow" => "❄️",
        "rain" => "🌧️", "wind" => "💨", "wave" => "🌊", "leaf" => "🍃", "tree" => "🌲",
        "rose" => "🌹", "pine" => "🌲", "oak" => "🌳", "fern" => "🌿", "moss" => "🌱",
        "cloud" => "☁️", "storm" => "⛈️", "frost" => "🥶", "coral" => "🪸",
        "gem" => "💎", "jade" => "💚", "pearl" => "🦪", "amber" => "🟠",
        "lake" => "🏞️", "river" => "🏞️", "creek" => "💧", "brook" => "💧",
        "rock" => "🪨", "stone" => "🪨", "cliff" => "🏔️", "peak" => "⛰️", "summit" => "🏔️",
        "meadow" => "🌾", "grove" => "🌳", "marsh" => "🌿",
        // Generic fallbacks by category (remaining animals)
        "elk" | "hen" | "jay" | "yak" | "lamb" | "mole" | "puma" | "seal" |
        "slug" | "toad" | "wasp" | "worm" | "crane" | "finch" | "gecko" |
        "heron" | "lemur" | "skunk" | "stork" | "trout" | "viper" | "badger" |
        "beetle" | "bobcat" | "ferret" | "gopher" | "magpie" | "mantis" |
        "marmot" | "osprey" | "pelican" | "pigeon" | "racoon" | "salmon" | "walrus" | "weasel" => "🐾",
        // Generic fallbacks (remaining nature)
        "ash" | "bay" | "elm" | "fir" | "ice" | "ivy" | "ore" | "beam" | "bolt" |
        "clay" | "coal" | "cone" | "cork" | "dune" | "foam" | "glen" | "hail" |
        "lime" | "mist" | "pond" | "reed" | "root" | "sand" | "tide" | "vale" |
        "vine" | "birch" | "cedar" | "delta" | "flint" | "maple" | "plum" |
        "ridge" | "shore" | "slate" | "steel" | "thorn" | "willow" | "crystal" | "pebble" => "🌿",
        // Default
        _ => "💫",
    }
}

/// Generate a memorable session name like "swift-fox" or "blue-whale"
/// Returns (full_id, short_name) where:
/// - full_id is the storage identifier like "session_swift-fox_1234567890"
/// - short_name is the memorable part like "swift-fox"
pub fn new_memorable_session_id() -> (String, String) {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();

    // Use the random value to pick a word
    let word_idx = (rand as usize) % WORDS.len();
    let word = WORDS[word_idx];

    // The short name is just the word
    let short_name = word.to_string();

    // Full ID includes timestamp for uniqueness
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

        // Short name should be a valid word
        assert!(WORDS.contains(&short_name.as_str()));
    }

    #[test]
    fn test_extract_session_name() {
        assert_eq!(extract_session_name("session_fox_1234567890"), Some("fox"));
        assert_eq!(extract_session_name("session_blue-whale_1234567890"), Some("blue-whale"));
        assert_eq!(extract_session_name("session_1234567890_9876543210"), Some("1234567890"));
        assert_eq!(extract_session_name("invalid"), None);
        assert_eq!(extract_session_name("session_"), None);
    }

    #[test]
    fn test_unique_session_ids() {
        let (id1, _) = new_memorable_session_id();
        let (id2, _) = new_memorable_session_id();

        // Even with same word, timestamps should differ
        assert_ne!(id1, id2);
    }
}
