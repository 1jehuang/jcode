//! Version-matched documentation embedded independently of application code.
//!
//! Keep the corpus private and access it through a non-inlined function. Docs
//! edits should not put their contents into the application core's expanded
//! source or require downstream crates to optimize an accessor again.

include!(concat!(env!("OUT_DIR"), "/jcode_docs.rs"));

/// The README and top-level Markdown documents shipped with this build.
#[inline(never)]
pub fn documents() -> &'static [(&'static str, &'static str)] {
    JCODE_DOCS
}

#[cfg(test)]
mod tests {
    use super::documents;

    #[test]
    fn corpus_is_sorted_unique_and_excludes_plans() {
        let docs = documents();
        assert!(docs.iter().any(|(path, _)| *path == "README.md"));
        assert!(docs.iter().any(|(path, _)| *path == "docs/README.md"));
        assert!(docs.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(docs.iter().all(|(path, body)| {
            !body.is_empty()
                && (*path == "README.md"
                    || (path.starts_with("docs/")
                        && path.ends_with(".md")
                        && !path[5..].contains('/')))
        }));
    }

    #[test]
    fn corpus_matches_build_inputs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (path, body) in documents() {
            assert_eq!(*body, std::fs::read_to_string(root.join(path)).unwrap());
        }
        let markdown_files = std::fs::read_dir(root.join("docs"))
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| {
                entry.path().is_file() && entry.path().extension().is_some_and(|ext| ext == "md")
            })
            .count();
        assert_eq!(documents().len(), markdown_files + 1);
    }
}
