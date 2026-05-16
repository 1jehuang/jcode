//! Incremental Tree-sitter Parser — O(changes) reparse using tree-sitter's edit API.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// An edit range description.
#[derive(Debug, Clone)]
pub struct SourceEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_text: String,
}

/// Incremental tree-sitter parser with tree caching.
pub struct IncrementalParser {
    cache: Arc<RwLock<HashMap<PathBuf, (String, tree_sitter::Tree)>>>,
    parser: Arc<RwLock<tree_sitter::Parser>>,
    stats: Arc<RwLock<IncrementalStats>>,
}

#[derive(Debug, Clone, Default)]
pub struct IncrementalStats {
    pub full_parses: u64,
    pub incremental_parses: u64,
    pub total_time_ms: u64,
}

impl IncrementalParser {
    pub fn new() -> Self {
        let mut parser = tree_sitter::Parser::new();
        let _ = parser.set_language(&tree_sitter_rust::LANGUAGE.into());
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            parser: Arc::new(RwLock::new(parser)),
            stats: Arc::new(RwLock::new(IncrementalStats::default())),
        }
    }

    /// Parse source with incremental support.
    /// Uses tree-sitter's edit() + parse() for O(changes) reparse.
    pub async fn parse(
        &self,
        path: &PathBuf,
        new_source: &str,
    ) -> anyhow::Result<tree_sitter::Tree> {
        let start = std::time::Instant::now();

        let cached = { self.cache.read().await.get(path).cloned() };

        let (tree, is_incr) = match cached {
            Some((old_source, old_tree)) if !old_source.is_empty() => {
                if let Some(input_edit) = compute_input_edit(&old_source, new_source) {
                    let mut tree = old_tree;
                    tree.edit(&input_edit);
                    let mut p = self.parser.write().await;
                    match p.parse(new_source, Some(&tree)) {
                        Some(t) => (t, true),
                        None => {
                            let mut p = self.parser.write().await;
                            (p.parse(new_source, None).ok_or_else(|| anyhow::anyhow!("parse failed"))?, false)
                        }
                    }
                } else {
                    (old_tree, true)
                }
            }
            _ => {
                let mut p = self.parser.write().await;
                (p.parse(new_source, None).ok_or_else(|| anyhow::anyhow!("parse failed"))?, false)
            }
        };

        self.cache.write().await.insert(path.clone(), (new_source.to_string(), tree.clone()));

        let mut s = self.stats.write().await;
        s.total_time_ms += start.elapsed().as_millis() as u64;
        if is_incr { s.incremental_parses += 1; } else { s.full_parses += 1; }

        Ok(tree)
    }

    /// Full reparse (no incremental), updates cache.
    pub async fn parse_full(&self, path: &PathBuf, source: &str) -> anyhow::Result<tree_sitter::Tree> {
        let mut p = self.parser.write().await;
        let tree = p.parse(source, None).ok_or_else(|| anyhow::anyhow!("parse failed"))?;
        self.cache.write().await.insert(path.clone(), (source.to_string(), tree.clone()));
        Ok(tree)
    }

    pub async fn invalidate(&self, path: &PathBuf) { self.cache.write().await.remove(path); }
    pub async fn clear(&self) { self.cache.write().await.clear(); }
    pub async fn stats(&self) -> IncrementalStats { self.stats.read().await.clone() }
    pub async fn cache_size(&self) -> usize { self.cache.read().await.len() }
}

impl Default for IncrementalParser { fn default() -> Self { Self::new() } }

/// Compute a tree-sitter `InputEdit` between old and new text.
/// Uses common prefix/suffix algorithm (O(n)).
fn compute_input_edit(old: &str, new: &str) -> Option<tree_sitter::InputEdit> {
    if old == new { return None; }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let (ol, nl) = (ob.len(), nb.len());

    // Common prefix bytes
    let pref = ob.iter().zip(nb.iter()).take_while(|(a,b)| a==b).count();

    // Common suffix bytes (after prefix region)
    let max_suf = ol.min(nl).saturating_sub(pref);
    let mut suf = 0usize;
    for i in 1..=max_suf {
        if ob[ol - i] == nb[nl - i] { suf = i; } else { break; }
    }

    let sb = pref;
    let oeb = ol.saturating_sub(suf);
    let neb = nl.saturating_sub(suf);

    let n_old = ob[sb..oeb].iter().filter(|&&b| b == b'\n').count() as usize;
    let n_new = nb[sb..neb].iter().filter(|&&b| b == b'\n').count() as usize;

    let start_row = old[..sb].chars().filter(|&c| c == '\n').count();
    let start_col = sb - old[..sb].rfind('\n').map(|i| i+1).unwrap_or(0);

    let old_end_row = start_row + n_old;
    let old_end_col = oeb - old[..oeb].rfind('\n').map(|i| i+1).unwrap_or(0);
    let new_end_row = start_row + n_new;
    let new_end_col = neb - new[..neb].rfind('\n').map(|i| i+1).unwrap_or(0);

    Some(tree_sitter::InputEdit {
        start_byte: sb,
        old_end_byte: oeb,
        new_end_byte: neb,
        start_position: tree_sitter::Point { row: start_row, column: start_col },
        old_end_position: tree_sitter::Point { row: old_end_row, column: old_end_col },
        new_end_position: tree_sitter::Point { row: new_end_row, column: new_end_col },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_incremental_parse_twice() {
        let p = IncrementalParser::new();
        let path = PathBuf::from("test.rs");
        let code1 = "fn main() { let x = 1; }";
        let _t1 = p.parse(&path, code1).await.unwrap();
        let code2 = "fn main() { let x = 42; }";
        let _t2 = p.parse(&path, code2).await.unwrap();
        let s = p.stats().await;
        assert_eq!(s.full_parses, 1);
        assert_eq!(s.incremental_parses, 1);
    }

    #[test]
    fn test_compute_edit_simple() {
        let e = compute_input_edit("fn a() {}", "fn a() { return 1; }");
        assert!(e.is_some());
        let e = e.unwrap();
        assert!(e.start_byte > 0);
        assert!(e.old_end_byte > e.start_byte);
        assert!(e.new_end_byte > e.old_end_byte);
    }

    #[test]
    fn test_compute_edit_no_change() {
        assert!(compute_input_edit("hello", "hello").is_none());
    }
}
