use crate::parallel_processor::ProcessedFile;
use similar::ChangeTag;

/// A merged unified diff across multiple files.
#[derive(Debug, Clone)]
pub struct UnifiedDiff {
    pub files: Vec<UnifiedDiffFile>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

impl UnifiedDiff {
    pub fn empty() -> Self {
        Self { files: Vec::new(), total_additions: 0, total_deletions: 0 }
    }
}

/// Diff for a single file within a unified result.
#[derive(Debug, Clone)]
pub struct UnifiedDiffFile {
    pub path: String,
    pub diff_text: String,
    pub additions: usize,
    pub deletions: usize,
}

/// Merge multiple ProcessedFiles into a single unified diff.
pub fn merge_diffs(processed: &[ProcessedFile]) -> UnifiedDiff {
    let files: Vec<UnifiedDiffFile> = processed
        .iter()
        .map(|pf| {
            let (additions, deletions) = pf.hunks.iter().flat_map(|h| &h.changes).fold(
                (0usize, 0usize),
                |(add, del), (tag, text)| match tag {
                    ChangeTag::Insert => (add + text.lines().count(), del),
                    ChangeTag::Delete => (add, del + text.lines().count()),
                    ChangeTag::Equal => (add, del),
                },
            );
            let diff_text = format_diff_text(pf);
            UnifiedDiffFile {
                path: pf.path.clone(),
                diff_text,
                additions,
                deletions,
            }
        })
        .collect();

    let total_additions: usize = files.iter().map(|f| f.additions).sum();
    let total_deletions: usize = files.iter().map(|f| f.deletions).sum();

    UnifiedDiff { files, total_additions, total_deletions }
}

fn format_diff_text(pf: &ProcessedFile) -> String {
    let mut out = format!("--- a/{}\n+++ b/{}\n", pf.path, pf.path);
    for hunk in &pf.hunks {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_end - hunk.old_start,
            hunk.new_start, hunk.new_end - hunk.new_start
        ));
        for (tag, text) in &hunk.changes {
            match tag {
                ChangeTag::Insert => out.push_str(&format!("+{}", text)),
                ChangeTag::Delete => out.push_str(&format!("-{}", text)),
                ChangeTag::Equal => out.push_str(&format!(" {}", text)),
            }
            if !text.ends_with('\n') { out.push('\n'); }
        }
    }
    out
}
