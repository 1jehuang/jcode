use super::*;

/// Query-independent search inputs for one session. Shared with the worker
/// so that a keystroke never copies the metadata index on the UI thread.
pub(super) struct Entry {
    pub(super) id: String,
    pub(super) paths: Vec<PathBuf>,
    pub(super) search_index: String,
    pub(super) title: String,
    pub(super) short_name: String,
    pub(super) save_label: Option<String>,
    pub(super) has_prompt: bool,
    pub(super) opening_prompt: String,
    pub(super) intent_text: String,
    pub(super) working_dir: Option<String>,
    pub(super) server_name: Option<String>,
    pub(super) cloud: bool,
}

/// Bytes compared at each end of an index to detect an in-place refresh.
const INDEX_PROBE_BYTES: usize = 64;

impl Entry {
    pub(super) fn new(session: &SessionInfo) -> Self {
        Self {
            id: session.id.clone(),
            paths: transcript_paths(session),
            search_index: session.search_index.clone(),
            title: session.title.clone(),
            short_name: session.short_name.clone(),
            save_label: session.save_label.clone(),
            working_dir: session.working_dir.clone(),
            server_name: session.server_name.clone(),
            cloud: session.server_icon.as_deref() == Some("☁"),
            has_prompt: session.first_user_prompt.is_some(),
            opening_prompt: session
                .first_user_prompt
                .as_deref()
                .unwrap_or("")
                .to_owned(),
            intent_text: format!(
                "{}\n{}\n{}",
                session.title,
                session.save_label.as_deref().unwrap_or(""),
                session.first_user_prompt.as_deref().unwrap_or(""),
            ),
        }
    }

    /// Cheap check that this entry still reflects `session`, so an unchanged
    /// history is reused across keystrokes without comparing whole indexes.
    pub(super) fn describes(&self, session: &SessionInfo) -> bool {
        let (old, new) = (
            self.search_index.as_bytes(),
            session.search_index.as_bytes(),
        );
        let probe = INDEX_PROBE_BYTES.min(old.len());
        self.id == session.id
            && old.len() == new.len()
            && old[..probe] == new[..probe]
            && old[old.len() - probe..] == new[new.len() - probe..]
            && self.title == session.title
            && self.short_name == session.short_name
            && self.save_label == session.save_label
            && self.working_dir == session.working_dir
            && self.server_name == session.server_name
            && self.cloud == (session.server_icon.as_deref() == Some("☁"))
            && self.has_prompt == session.first_user_prompt.is_some()
            && self.opening_prompt == session.first_user_prompt.as_deref().unwrap_or("")
    }

    pub(super) fn exact_field(&self, query: &str) -> bool {
        [&self.title, &self.short_name, &self.id]
            .iter()
            .any(|field| normalized_equals(field, query))
            || self
                .save_label
                .as_ref()
                .is_some_and(|label| normalized_equals(label, query))
    }

    pub(super) fn identity_rank(&self, tokens: &[String]) -> (bool, bool, bool) {
        super::super::identity_search::rank_fields(
            [
                &self.id,
                &self.short_name,
                &self.title,
                self.save_label.as_deref().unwrap_or(""),
                self.working_dir.as_deref().unwrap_or(""),
                self.server_name.as_deref().unwrap_or(""),
            ],
            self.cloud,
            tokens,
        )
    }
}

pub(super) struct Candidate {
    pub(super) entry: Arc<Entry>,
    pub(super) missing_tokens: Vec<String>,
    pub(super) exact_field: bool,
    /// Per-token origin evidence from metadata, before transcript reads.
    pub(super) prior: Vec<Option<Source>>,
}

impl std::ops::Deref for Candidate {
    type Target = Entry;
    fn deref(&self) -> &Entry {
        &self.entry
    }
}

/// Decide which sessions need worker evidence for `query`. With a deferred
/// (large) history this runs on the worker, never on the keystroke.
pub(super) fn build_candidates(
    corpus: &[Arc<Entry>],
    query: &str,
    tokens: &[String],
    defer_metadata: bool,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for entry in corpus {
        let missing_tokens: Vec<_> = tokens
            .iter()
            .filter(|token| defer_metadata || !entry.search_index.contains(token.as_str()))
            .cloned()
            .collect();
        // Literal metadata matches still go to the worker, which upgrades
        // their rank once it knows where in the transcript each word came from,
        // unless every word already comes from a user-intent field.
        if !defer_metadata && missing_tokens.is_empty() && (tokens.len() < 4 || !entry.has_prompt) {
            let intent = lowercase_field(&entry.intent_text);
            if tokens.iter().all(|token| intent.contains(token.as_str())) {
                continue;
            }
        }
        candidates.push(Candidate {
            entry: entry.clone(),
            missing_tokens,
            exact_field: entry.exact_field(query),
            prior: Vec::new(),
        });
    }
    candidates
}
