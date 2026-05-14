use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Locale {
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub direction: LanguageDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LanguageDirection {
    LTR,  // Left-to-Right (从左到右)
    RTL,  // Right-to-Left (从右到左)
}

impl Locale {
    pub fn new(code: &str, name: &str, native_name: &str, direction: LanguageDirection) -> Self {
        Locale {
            code: code.to_string(),
            name: name.to_string(),
            native_name: native_name.to_string(),
            direction,
        }
    }

    pub fn is_rtl(&self) -> bool { self.direction == LanguageDirection::RTL }
}

impl Default for Locale {
    fn default() -> Self {
        Locale::new("en", "English", "English", LanguageDirection::LTR)
    }
}

pub fn supported_locales() -> Vec<Locale> {
    vec![
        Locale::new("en", "English", "English", LanguageDirection::LTR),
        Locale::new("zh-CN", "Chinese Simplified", "简体中文", LanguageDirection::LTR),
        Locale::new("zh-TW", "Chinese Traditional", "繁體中文", LanguageDirection::LTR),
        Locale::new("ja", "Japanese", "日本語", LanguageDirection::LTR),
        Locale::new("ko", "Korean", "한국어", LanguageDirection::LTR),
        Locale::new("es", "Spanish", "Español", LanguageDirection::LTR),
        Locale::new("fr", "French", "Français", LanguageDirection::LTR),
        Locale::new("de", "German", "Deutsch", LanguageDirection::LTR),
        Locale::new("ru", "Russian", "Русский", LanguageDirection::LTR),
        Locale::new("pt", "Portuguese", "Português", LanguageDirection::LTR),
        Locale::new("it", "Italian", "Italiano", LanguageDirection::LTR),
        Locale::new("nl", "Dutch", "Nederlands", LanguageDirection::LTR),
        Locale::new("pl", "Polish", "Polski", LanguageDirection::LTR),
        Locale::new("ar", "Arabic", "العربية", LanguageDirection::RTL),
        Locale::new("hi", "Hindi", "हिन्दी", LanguageDirection::LTR),
        Locale::new("tr", "Turkish", "Türkçe", LanguageDirection::LTR),
        Locale::new("vi", "Vietnamese", "Tiếng Việt", LanguageDirection::LTR),
        Locale::new("th", "Thai", "ไทย", LanguageDirection::LTR),
        Locale::new("id", "Indonesian", "Bahasa Indonesia", LanguageDirection::LTR),
        Locale::new("sv", "Swedish", "Svenska", LanguageDirection::LTR),
    ]
}
