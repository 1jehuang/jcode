use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::HashSet;

use std::sync::OnceLock;

/// Version-matched documentation supplied by the final executable.
pub type BundledDocs = &'static [(&'static str, &'static str)];

static DOCUMENTS: OnceLock<BundledDocs> = OnceLock::new();

/// Install the executable's immutable documentation corpus at startup.
///
/// Keeping the data at the composition root means a documentation edit does
/// not invalidate the application core or the TUI. Repeated startup setup is
/// harmless: the first corpus remains authoritative for this process.
pub fn register_bundled_docs(docs: BundledDocs) {
    let _ = DOCUMENTS.set(docs);
}

/// The executable's registered corpus, or an explicit setup error.
pub fn bundled_docs() -> Result<BundledDocs> {
    // Unit tests exercise the real corpus, without making normal app-core or
    // TUI builds depend on that changing data.
    #[cfg(test)]
    DOCUMENTS.get_or_init(jcode_docs_data::documents);
    DOCUMENTS
        .get()
        .copied()
        .ok_or_else(|| anyhow!("bundled Jcode documentation was not registered by the executable"))
}

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 10;
const MAX_SECTION_CHARS: usize = 4_000;

pub struct JcodeDocsTool;

impl JcodeDocsTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct JcodeDocsInput {
    #[serde(default = "default_action")]
    action: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn default_action() -> String {
    "search".to_string()
}

#[derive(Debug)]
struct Section<'a> {
    path: &'a str,
    heading: String,
    body: String,
}

#[async_trait]
impl Tool for JcodeDocsTool {
    fn name(&self) -> &str {
        "jcode_docs"
    }

    fn description(&self) -> &str {
        "Search bundled Jcode docs. Use first for questions about Jcode itself."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["search", "read", "list"],
                    "description": "Search documentation (default), read one document, or list bundled documents."
                },
                "query": {
                    "type": "string",
                    "description": "Words or question to search for. Required for search."
                },
                "path": {
                    "type": "string",
                    "description": "Exact bundled path returned by search/list. Required for read."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "description": "Maximum search results. Defaults to 5."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: JcodeDocsInput = serde_json::from_value(input)?;
        let docs = bundled_docs()?;
        let output = match params.action.as_str() {
            "search" => search(
                docs,
                params
                    .query
                    .as_deref()
                    .ok_or_else(|| anyhow!("query is required for search"))?,
                params.limit,
            ),
            "read" => read_doc(
                docs,
                params
                    .path
                    .as_deref()
                    .ok_or_else(|| anyhow!("path is required for read"))?,
            )?,
            "list" => list_docs(docs),
            other => {
                return Err(anyhow!(
                    "unknown action {other:?}; use search, read, or list"
                ));
            }
        };
        Ok(ToolOutput::new(output).with_title(format!("jcode docs {}", params.action)))
    }
}

fn list_docs(docs: BundledDocs) -> String {
    let mut output = format!("Bundled Jcode documentation ({} files):\n", docs.len());
    for (path, body) in docs {
        let title = body
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .unwrap_or(path);
        output.push_str(&format!("- `{path}`: {title}\n"));
    }
    output
}

fn read_doc(docs: BundledDocs, path: &str) -> Result<String> {
    let (_, body) = docs
        .iter()
        .find(|(candidate, _)| *candidate == path)
        .ok_or_else(|| {
            anyhow!("documentation path not found: {path}. Use action=list to see available paths.")
        })?;
    Ok(format!(
        "Source: `{path}` (bundled with this Jcode build)\n\n{body}"
    ))
}

fn search(docs: BundledDocs, query: &str, limit: Option<usize>) -> String {
    let terms = terms(query);
    if terms.is_empty() {
        return "Search query must contain at least one word.".to_string();
    }
    let mut matches = sections(docs)
        .into_iter()
        .filter_map(|section| {
            let heading = section.heading.to_lowercase();
            let body = section.body.to_lowercase();
            let path = section.path.to_lowercase();
            let matched = terms
                .iter()
                .filter(|term| {
                    heading.contains(*term) || body.contains(*term) || path.contains(*term)
                })
                .count();
            if matched == 0 {
                return None;
            }
            let occurrences = terms
                .iter()
                .map(|term| body.matches(term.as_str()).count().min(10))
                .sum::<usize>();
            let score = matched * 100
                + terms.iter().filter(|term| heading.contains(*term)).count() * 40
                + terms.iter().filter(|term| path.contains(*term)).count() * 20
                + occurrences;
            Some((score, section))
        })
        .collect::<Vec<_>>();
    matches
        .sort_by_key(|(score, section)| (Reverse(*score), section.path, section.heading.clone()));
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut output = format!("Jcode docs results for {query:?} (bundled with this Jcode build):\n");
    for (index, (_, section)) in matches.into_iter().take(limit).enumerate() {
        let excerpt = relevant_excerpt(&section.body, &terms);
        output.push_str(&format!(
            "\n{}. `{}` > {}\n{}\n",
            index + 1,
            section.path,
            section.heading,
            excerpt
        ));
    }
    if output.lines().count() == 1 {
        output.push_str("\nNo matching documentation. Try fewer or broader terms.\n");
    }
    output
}

fn terms(query: &str) -> Vec<String> {
    let stop: HashSet<&str> = [
        "a", "an", "and", "about", "does", "for", "how", "i", "in", "is", "jcode", "of", "on",
        "the", "to", "what", "with",
    ]
    .into_iter()
    .collect();
    query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .map(str::to_lowercase)
        .filter(|term| term.len() > 1 && !stop.contains(term.as_str()))
        .collect()
}

fn sections(docs: BundledDocs) -> Vec<Section<'static>> {
    let mut result = Vec::new();
    for (path, document) in docs {
        let mut heading = "Overview".to_string();
        let mut body = String::new();
        for line in document.lines() {
            if line.starts_with('#') {
                if !body.trim().is_empty() {
                    result.push(Section {
                        path,
                        heading,
                        body: std::mem::take(&mut body),
                    });
                }
                heading = line.trim_start_matches('#').trim().to_string();
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        if !body.trim().is_empty() {
            result.push(Section {
                path,
                heading,
                body,
            });
        }
    }
    result
}

fn relevant_excerpt(body: &str, terms: &[String]) -> String {
    let paragraphs = body
        .split("\n\n")
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    let best = paragraphs
        .iter()
        .max_by_key(|part| {
            let lower = part.to_lowercase();
            terms.iter().filter(|term| lower.contains(*term)).count()
        })
        .copied()
        .unwrap_or(body)
        .trim();
    if best.chars().count() <= MAX_SECTION_CHARS {
        best.to_string()
    } else {
        format!(
            "{}…",
            best.chars().take(MAX_SECTION_CHARS).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_includes_current_docs_but_not_plans() {
        let docs = bundled_docs().unwrap();
        assert!(docs.iter().any(|(path, _)| *path == "README.md"));
        assert!(docs.iter().any(|(path, _)| *path == "docs/README.md"));
        assert!(!docs.iter().any(|(path, _)| path.starts_with("docs/plans/")));
    }

    #[test]
    fn search_finds_relevant_version_matched_documentation() {
        let output = search(
            bundled_docs().unwrap(),
            "How does swarm coordination work?",
            Some(3),
        );
        assert!(output.contains("docs/SWARM_ARCHITECTURE.md"), "{output}");
        assert!(output.contains("bundled with this Jcode build"));
    }

    #[test]
    fn exact_document_can_be_read() {
        let output = read_doc(bundled_docs().unwrap(), "docs/README.md").unwrap();
        assert!(output.contains("# jcode Docs"));
    }
}
