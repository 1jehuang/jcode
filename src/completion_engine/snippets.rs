use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    pub prefix: String,
    pub body: String,
    pub description: String,
    pub language: String,
}

pub struct SnippetStore {
    snippets: HashMap<String, Vec<Snippet>>,
}

impl SnippetStore {
    pub fn new() -> Self {
        Self {
            snippets: HashMap::new(),
        }
    }

    pub fn add_snippet(&mut self, language: &str, snippet: Snippet) {
        self.snippets.entry(language.to_string()).or_insert_with(Vec::new).push(snippet);
    }

    pub fn get_snippets(&self, language: &str) -> Vec<Snippet> {
        self.snippets.get(language).cloned().unwrap_or_default()
    }

    pub fn get_all_snippets(&self) -> Vec<Snippet> {
        self.snippets.values().flatten().cloned().collect()
    }
}