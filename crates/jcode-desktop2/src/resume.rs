//! Resume from disk: the stored sessions, grouped by the project they ran in.
//!
//! The strip and the overview only know *live* sessions, because that is all
//! the daemon volunteers. Everything the user did last week is on disk and
//! unreachable from this window, which makes the app feel like it forgets: the
//! TUI's `/resume` is the surface that fixes that, and this is its desktop
//! counterpart.
//!
//! Two halves, kept apart on purpose:
//!
//! - [`scan`] reads `~/.jcode/sessions` and is the only part that touches the
//!   filesystem. It is called on a worker thread, so a directory with fifty
//!   thousand records in it can never stall a frame.
//! - [`Picker`] is pure state: the grouped rows, where the highlight sits, and
//!   what the user has typed to narrow it. That makes the whole navigation
//!   testable without a window, which is the same bargain [`crate::overview`]
//!   makes.
//!
//! The preview is not here. Hovering a row asks the harness to peek the
//! session, and the reply lands in [`crate::overview::Peeks`], so a stored
//! session and a live one are previewed by exactly one code path.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many stored sessions are read per scan, newest first.
///
/// The records directory is append-only and unbounded (55k files on the
/// author's machine), so a picker that read all of them would spend seconds
/// parsing transcripts from months ago that nobody will ever scroll to. The
/// newest few hundred is what "resume" actually means.
pub const SCAN_LIMIT: usize = 400;

/// Bytes read from each end of a record.
///
/// A session record is one JSON object whose `messages` array sits in the
/// middle: the identity fields (`id`, `title`, `short_name`) are at the head
/// and `working_dir` is written after the messages, so both ends are cheap
/// reads and the megabytes between them are never touched.
const EDGE_BYTES: u64 = 8 * 1024;

/// One stored session, as the picker needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub session_id: String,
    /// Directory the session ran in, when the record names one.
    pub working_dir: Option<String>,
    /// The user's title, when they set one.
    pub title: Option<String>,
    /// Size of the record, a cheap proxy for how much conversation is in it.
    pub bytes: u64,
    /// Last modification time, which is what the list is ordered by.
    pub modified: SystemTime,
}

impl Record {
    /// The name a human would use for this session.
    pub fn label(&self) -> String {
        match self.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => title.to_string(),
            _ => crate::overview::short_id(&self.session_id),
        }
    }
}

/// `~/.jcode/sessions`, or `$JCODE_HOME/sessions` when the home is overridden.
///
/// Resolved the same way the bridge resolves it, so the picker and the daemon
/// can never disagree about which directory holds the sessions.
pub fn sessions_dir() -> Option<PathBuf> {
    let home = match std::env::var_os("JCODE_HOME") {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".jcode"),
    };
    Some(home.join("sessions"))
}

/// Read the newest `limit` stored sessions from `dir`, newest first.
///
/// Best effort throughout: an unreadable or half-written record is skipped
/// rather than failing the scan, because the alternative is an empty picker
/// whenever one file out of hundreds is mid-write.
pub fn scan(dir: &Path, limit: usize) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // Only the canonical records: `.bak` is a previous copy of one we
        // already list, and `.journal.jsonl` is the live append log.
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        candidates.push((modified, meta.len(), path));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.truncate(limit);
    candidates
        .into_iter()
        .filter_map(|(modified, bytes, path)| read_record(&path, bytes, modified))
        .collect()
}

/// Pull one record's identity out of its file without parsing the transcript.
fn read_record(path: &Path, bytes: u64, modified: SystemTime) -> Option<Record> {
    use std::io::{Read, Seek, SeekFrom};
    let session_id = path.file_stem()?.to_str()?.to_string();
    // A record with no messages at all is a session that never started; it
    // would be a row the user can only be disappointed by.
    if bytes == 0 {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; EDGE_BYTES.min(bytes) as usize];
    file.read_exact(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head).into_owned();

    let tail = if bytes > EDGE_BYTES {
        file.seek(SeekFrom::Start(bytes - EDGE_BYTES)).ok()?;
        let mut tail = vec![0u8; EDGE_BYTES as usize];
        file.read_exact(&mut tail).ok()?;
        String::from_utf8_lossy(&tail).into_owned()
    } else {
        head.clone()
    };

    Some(Record {
        session_id,
        // `working_dir` is written after the messages, so the tail is where it
        // is for a long session and the head doubles as the tail for a short
        // one. Both are consulted so neither shape loses its grouping.
        working_dir: field(&tail, "working_dir").or_else(|| field(&head, "working_dir")),
        title: field(&head, "title"),
        bytes,
        modified,
    })
}

/// Value of a string field in a JSON fragment, ignoring `null`.
///
/// A hand-rolled scan rather than a parse: the fragment is deliberately a
/// slice of a much larger object, so it is not valid JSON and `serde` cannot
/// help. Escapes are decoded only far enough for a path or a title, which is
/// all that is displayed.
fn field(fragment: &str, name: &str) -> Option<String> {
    let needle = format!("\"{name}\":");
    let start = fragment.find(&needle)? + needle.len();
    let rest = fragment[start..].trim_start();
    let mut chars = rest.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for (_, ch) in chars {
        match (escaped, ch) {
            (true, 'n') => {
                out.push('\n');
                escaped = false;
            }
            (true, 't') => {
                out.push('\t');
                escaped = false;
            }
            (true, other) => {
                out.push(other);
                escaped = false;
            }
            (false, '\\') => escaped = true,
            (false, '"') => return Some(out).filter(|value| !value.is_empty()),
            (false, other) => out.push(other),
        }
    }
    // Ran off the end of the fragment: the value was truncated by the window,
    // so it is not trustworthy as a label.
    None
}

/// A row in the picker's left panel: either a project or a session inside one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A working directory, with how many sessions it holds.
    Group {
        label: String,
        path: String,
        count: usize,
        expanded: bool,
    },
    /// A stored session, indented under its group.
    Session { index: usize },
}

/// The picker's state: what is on disk, what is open, and where focus sits.
///
/// Pure, and `Clone`, so a frame stays a function of the model and a test can
/// drive the whole gesture with no window and no filesystem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picker {
    open: bool,
    /// Stored sessions, newest first, as the last scan found them.
    records: Vec<Record>,
    /// Directories collapsed by the user. Everything else is expanded, so a
    /// fresh picker shows the sessions rather than a list of folders to open.
    collapsed: std::collections::BTreeSet<String>,
    /// What the user has typed to narrow the list.
    query: String,
    /// Index into [`Self::rows`] of the highlighted row.
    cursor: usize,
    /// Whether a scan is in flight, so the panel can say so rather than
    /// looking empty.
    scanning: bool,
}

/// Sessions whose directory is unknown are grouped under this label rather
/// than being dropped: an ungrouped session is still resumable.
const UNKNOWN_GROUP: &str = "(unknown project)";

impl Picker {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Open the picker. `scanning` records that a fresh scan was kicked off.
    pub fn open(&mut self, scanning: bool) {
        self.open = true;
        self.scanning = scanning;
        self.query.clear();
        self.cursor = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.scanning = false;
    }

    pub fn toggle(&mut self, scanning: bool) -> bool {
        match self.open {
            true => {
                self.close();
                false
            }
            false => {
                self.open(scanning);
                true
            }
        }
    }

    /// Adopt a completed scan, keeping the highlight on the same session when
    /// it survived: a rescan that moved the cursor would fight the user.
    pub fn set_records(&mut self, records: Vec<Record>) {
        let held = self.selected().map(|record| record.session_id.clone());
        self.records = records;
        self.scanning = false;
        self.cursor = 0;
        if let Some(held) = held {
            self.select_session(&held);
        }
        self.clamp();
    }

    /// The rows the left panel draws, in order.
    ///
    /// Rebuilt per call rather than cached: it is a walk over a few hundred
    /// records, and a cache here would be one more thing that can disagree
    /// with what was drawn when a scan lands mid-gesture.
    pub fn rows(&self) -> Vec<Row> {
        let query = self.query.trim().to_ascii_lowercase();
        // Groups in first-appearance order, which is newest-session-first
        // because the records are: the project you were last in is at the top,
        // where the hand already is.
        let mut order: Vec<String> = Vec::new();
        let mut members: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
        for (index, record) in self.records.iter().enumerate() {
            if !self.matches(record, &query) {
                continue;
            }
            let path = record
                .working_dir
                .clone()
                .unwrap_or_else(|| UNKNOWN_GROUP.to_string());
            if !members.contains_key(&path) {
                order.push(path.clone());
            }
            members.entry(path).or_default().push(index);
        }
        let mut rows = Vec::new();
        for path in order {
            let group = members.remove(&path).unwrap_or_default();
            // A search collapses nothing: the user is looking for a session,
            // not for a folder to open.
            let expanded = !query.is_empty() || !self.collapsed.contains(&path);
            rows.push(Row::Group {
                label: leaf(&path),
                path,
                count: group.len(),
                expanded,
            });
            if expanded {
                rows.extend(group.into_iter().map(|index| Row::Session { index }));
            }
        }
        rows
    }

    fn matches(&self, record: &Record, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let dir = record.working_dir.as_deref().unwrap_or_default();
        record.label().to_ascii_lowercase().contains(query)
            || record.session_id.to_ascii_lowercase().contains(query)
            || dir.to_ascii_lowercase().contains(query)
    }

    /// Index of the highlighted row.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The highlighted session, when the highlight is on one.
    pub fn selected(&self) -> Option<&Record> {
        match self.rows().get(self.cursor)? {
            Row::Session { index } => self.records.get(*index),
            Row::Group { .. } => None,
        }
    }

    /// Move the highlight by `delta` rows, stopping at the ends.
    ///
    /// No wrapping: a list this long reads as a scroll, and teleporting from
    /// the last row to the first looks like a bug rather than a convenience.
    pub fn move_cursor(&mut self, delta: isize) {
        let len = self.rows().len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, len as isize - 1) as usize;
    }

    /// Move the highlight to the first row of the next (or previous) group,
    /// which is how the panel is navigated when the list is long.
    pub fn move_group(&mut self, delta: isize) {
        let rows = self.rows();
        let groups: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::Group { .. }))
            .map(|(at, _)| at)
            .collect();
        if groups.is_empty() {
            return;
        }
        let current = groups
            .iter()
            .rposition(|at| *at <= self.cursor)
            .unwrap_or(0) as isize;
        let target = (current + delta).clamp(0, groups.len() as isize - 1) as usize;
        self.cursor = groups[target];
    }

    /// Put the highlight on a specific session, if the list still has it.
    pub fn select_session(&mut self, session_id: &str) -> bool {
        let rows = self.rows();
        for (at, row) in rows.iter().enumerate() {
            if let Row::Session { index } = row
                && self.records[*index].session_id == session_id
            {
                self.cursor = at;
                return true;
            }
        }
        false
    }

    /// Point the highlight at a row index, for pointer hover and clicks.
    pub fn set_cursor(&mut self, at: usize) {
        self.cursor = at;
        self.clamp();
    }

    /// Collapse the group the highlight is in, or the group itself.
    pub fn collapse(&mut self) {
        if let Some(path) = self.group_of_cursor() {
            self.collapsed.insert(path.clone());
            // Land on the group's own row: collapsing under the cursor would
            // otherwise leave the highlight pointing at a row that is gone.
            let rows = self.rows();
            if let Some(at) = rows
                .iter()
                .position(|row| matches!(row, Row::Group { path: group, .. } if *group == path))
            {
                self.cursor = at;
            }
            self.clamp();
        }
    }

    /// Expand the highlighted group, or step into it when already open.
    pub fn expand(&mut self) {
        if let Some(path) = self.group_of_cursor() {
            if self.collapsed.remove(&path) {
                return;
            }
            // Already open, so the natural next move is the first session.
            if matches!(self.rows().get(self.cursor), Some(Row::Group { .. })) {
                self.move_cursor(1);
            }
        }
    }

    /// Directory of whatever the highlight is on, group row or session row.
    fn group_of_cursor(&self) -> Option<String> {
        match self.rows().get(self.cursor)? {
            Row::Group { path, .. } => Some(path.clone()),
            Row::Session { index } => Some(
                self.records
                    .get(*index)?
                    .working_dir
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_GROUP.to_string()),
            ),
        }
    }

    /// Narrow the list. Typing resets the highlight to the top, so the best
    /// match is under Enter rather than wherever the cursor happened to be.
    pub fn type_char(&mut self, ch: char) {
        self.query.push(ch);
        self.cursor = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.cursor = 0;
    }

    fn clamp(&mut self) {
        let len = self.rows().len();
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }

    /// Build a picker in an exact state, for capture nodes and tests.
    ///
    /// Its own constructor rather than public fields: the state-space nodes
    /// need to pin a highlight without replaying a gesture, and the invariants
    /// (a clamped cursor, records that the cursor indexes) must hold for a
    /// pinned model exactly as for a driven one.
    pub fn pinned(records: Vec<Record>, cursor: usize, query: &str) -> Self {
        let mut picker = Self::default();
        picker.open(false);
        picker.set_records(records);
        picker.query = query.to_string();
        picker.cursor = cursor;
        picker.clamp();
        picker
    }

    /// A stored record, for capture nodes and tests.
    #[cfg(test)]
    pub fn record_at(&self, index: usize) -> Option<&Record> {
        self.records.get(index)
    }
}

/// Last component of a path, which is the name a human uses for a project.
pub fn leaf(path: &str) -> String {
    if path == UNKNOWN_GROUP {
        return path.to_string();
    }
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

/// A record's size, in the shortest form that still says how big it is.
pub fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        return format!("{bytes} B");
    }
    if value < KIB * KIB {
        return format!("{:.0} KB", value / KIB);
    }
    format!("{:.1} MB", value / (KIB * KIB))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, dir: Option<&str>, bytes: u64, secs: u64) -> Record {
        Record {
            session_id: id.to_string(),
            working_dir: dir.map(str::to_string),
            title: None,
            bytes,
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        }
    }

    fn picker() -> Picker {
        let mut picker = Picker::default();
        picker.open(false);
        picker.set_records(vec![
            record("session_fox_1_a", Some("/home/j/jcode"), 400, 30),
            record("session_owl_2_b", Some("/home/j/site"), 200, 20),
            record("session_bat_3_c", Some("/home/j/jcode"), 100, 10),
        ]);
        picker
    }

    /// The panel groups by directory, newest project first, with the sessions
    /// of a project under its own row.
    #[test]
    fn rows_group_sessions_under_their_project() {
        let rows = picker().rows();
        assert!(
            matches!(&rows[0], Row::Group { label, count, .. } if label == "jcode" && *count == 2)
        );
        assert!(matches!(rows[1], Row::Session { .. }));
        assert!(matches!(rows[2], Row::Session { .. }));
        assert!(matches!(&rows[3], Row::Group { label, .. } if label == "site"));
        assert!(matches!(rows[4], Row::Session { .. }));
    }

    /// Down moves to the first session, and the selection is a resumable
    /// record rather than a folder.
    #[test]
    fn the_highlight_walks_rows_and_selects_sessions() {
        let mut picker = picker();
        assert!(picker.selected().is_none(), "a group row is not a session");
        picker.move_cursor(1);
        assert_eq!(picker.selected().unwrap().session_id, "session_fox_1_a");
        picker.move_cursor(1);
        assert_eq!(picker.selected().unwrap().session_id, "session_bat_3_c");
    }

    /// The ends are walls: a long list must not teleport under the user.
    #[test]
    fn the_highlight_stops_at_the_ends() {
        let mut picker = picker();
        picker.move_cursor(-5);
        assert_eq!(picker.cursor(), 0);
        picker.move_cursor(100);
        assert_eq!(picker.cursor(), picker.rows().len() - 1);
    }

    /// Collapsing hides a project's sessions and leaves the highlight on the
    /// project, never on a row that no longer exists.
    #[test]
    fn collapsing_keeps_the_highlight_on_a_real_row() {
        let mut picker = picker();
        picker.move_cursor(1);
        picker.collapse();
        let rows = picker.rows();
        assert!(matches!(&rows[0], Row::Group { expanded, .. } if !*expanded));
        assert!(picker.cursor() < rows.len());
        assert!(matches!(rows[picker.cursor()], Row::Group { .. }));
        picker.expand();
        assert!(matches!(&picker.rows()[0], Row::Group { expanded, .. } if *expanded));
    }

    /// Typing narrows across name and directory, and a search ignores
    /// collapse: the user is hunting a session, not browsing folders.
    #[test]
    fn the_query_narrows_by_name_and_directory() {
        let mut picker = picker();
        picker.collapse();
        for ch in "owl".chars() {
            picker.type_char(ch);
        }
        let rows = picker.rows();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(matches!(&rows[0], Row::Group { label, .. } if label == "site"));
        picker.move_cursor(1);
        assert_eq!(picker.selected().unwrap().session_id, "session_owl_2_b");

        for _ in 0..3 {
            picker.backspace();
        }
        for ch in "jcode".chars() {
            picker.type_char(ch);
        }
        assert_eq!(picker.rows().len(), 3, "directory match lost");
    }

    /// A rescan must not move the highlight off the session the user is
    /// looking at, or hovering a preview would jump mid-read.
    #[test]
    fn a_rescan_keeps_the_selected_session() {
        let mut picker = picker();
        picker.move_cursor(2);
        let held = picker.selected().unwrap().session_id.clone();
        picker.set_records(vec![
            record("session_new_9_z", Some("/home/j/jcode"), 10, 90),
            record("session_fox_1_a", Some("/home/j/jcode"), 400, 30),
            record("session_bat_3_c", Some("/home/j/jcode"), 100, 10),
        ]);
        assert_eq!(picker.selected().map(|r| r.session_id.clone()), Some(held));
    }

    /// Ctrl+R twice returns to the conversation: the picker is a toggle, and
    /// closing must not leave the query behind for the next open.
    #[test]
    fn toggling_closes_and_forgets_the_query() {
        let mut picker = picker();
        picker.type_char('x');
        assert!(!picker.toggle(false));
        assert!(!picker.is_open());
        assert!(picker.toggle(false));
        assert!(picker.query().is_empty());
    }

    /// Sessions whose directory is unknown still get a row: an ungrouped
    /// session is resumable, and dropping it would silently lose history.
    #[test]
    fn sessions_without_a_directory_are_still_listed() {
        let mut picker = Picker::default();
        picker.open(false);
        picker.set_records(vec![record("session_ghost_1_a", None, 10, 1)]);
        let rows = picker.rows();
        assert!(matches!(&rows[0], Row::Group { label, .. } if label == UNKNOWN_GROUP));
        picker.move_cursor(1);
        assert_eq!(picker.selected().unwrap().session_id, "session_ghost_1_a");
    }

    /// The scan reads real records: identity from the head, directory from the
    /// tail, and it must survive a file it cannot use.
    #[test]
    fn scanning_reads_identity_and_directory_from_a_record() {
        let dir = std::env::temp_dir().join(format!("jcode-resume-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A realistic record: the directory is written *after* a big messages
        // array, which is why the scan reads both ends.
        let filler = "x".repeat(64 * 1024);
        std::fs::write(
            dir.join("session_fox_1_a.json"),
            format!(
                r#"{{"id":"session_fox_1_a","title":"a name","messages":["{filler}"],"working_dir":"/home/j/jcode"}}"#
            ),
        )
        .unwrap();
        // Noise that must not become rows: a backup, a journal, and an empty
        // file.
        std::fs::write(dir.join("session_fox_1_a.bak"), "{}").unwrap();
        std::fs::write(dir.join("session_owl_2_b.journal.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.join("session_empty_3_c.json"), "").unwrap();

        let records = scan(&dir, SCAN_LIMIT);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].session_id, "session_fox_1_a");
        assert_eq!(records[0].working_dir.as_deref(), Some("/home/j/jcode"));
        assert_eq!(records[0].label(), "a name");
    }

    /// A missing directory is a first run, not a crash.
    #[test]
    fn scanning_a_missing_directory_is_empty() {
        assert!(scan(Path::new("/nonexistent/jcode/sessions"), SCAN_LIMIT).is_empty());
    }

    /// A record with no title falls back to the generated session name, which
    /// is the only part of an id worth reading.
    #[test]
    fn the_label_falls_back_to_the_session_name() {
        assert_eq!(
            record("session_clover_1785130341680_5a8db08", None, 0, 0).label(),
            "clover"
        );
    }
}
