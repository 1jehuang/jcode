use std::collections::HashMap;
use super::locale::{Locale, supported_locales};
use super::translations::get_translations;

pub struct Translator {
    current_locale: Locale,
    translations: HashMap<String, String>,
    fallback_locale: Locale,
}

impl Translator {
    pub fn new(locale_code: &str) -> Self {
        let locale = Self::find_locale(locale_code).unwrap_or_default();
        let translations = get_translations(&locale.code);

        Translator {
            current_locale: locale.clone(),
            translations,
            fallback_locale: Locale::default(),
        }
    }

    pub fn set_locale(&mut self, locale_code: &str) {
        if let Some(locale) = Self::find_locale(locale_code) {
            self.current_locale = locale.clone();
            self.translations = get_translations(&locale.code);
        }
    }

    pub fn get_locale(&self) -> &Locale { &self.current_locale }
    pub fn get_direction(&self) -> &super::locale::LanguageDirection { &self.current_locale.direction }

    pub fn t(&self, key: &str) -> String {
        self.translations
            .get(key)
            .cloned()
            .unwrap_or_else(|| {
                get_translations(&self.fallback_locale.code)
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| key.to_string())
            })
    }

    pub fn t_with_args(&self, key: &str, args: &[&str]) -> String {
        let mut result = self.t(key);
        for (i, arg) in args.iter().enumerate() {
            result = result.replace(&format!("{{{}}}", i), arg);
        }
        result
    }

    pub fn t_named_args(&self, key: &str, args: &HashMap<&str, &str>) -> String {
        let mut result = self.t(key);
        for (name, value) in args {
            result = result.replace(&format!("{{{{{}}}}}", name), value);
        }
        result
    }

    pub fn pluralize(&self, key: &str, count: usize) -> String {
        let template_key = if count == 1 { format!("{}.one", key) } else { format!("{}.other", key) };
        self.t_with_args(&template_key, &[&count.to_string()])
    }

    pub fn has_translation(&self, key: &str) -> bool {
        self.translations.contains_key(key)
    }

    pub fn available_locales() -> Vec<Locale> { supported_locales() }

    fn find_locale(code: &str) -> Option<Locale> {
        supported_locales().into_iter().find(|l| l.code == code)
    }
}
