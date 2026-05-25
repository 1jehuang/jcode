use std::sync::OnceLock;

/// A changelog entry: hash, optional version tag, and commit subject.
#[derive(Clone, Copy)]
pub(super) struct ChangelogEntry<'a> {
    pub hash: &'a str,
    pub tag: &'a str,
    pub timestamp: Option<i64>,
    pub subject: &'a str,
}

/// A group of changelog entries under a version heading.
#[derive(Clone)]
pub(super) struct ChangelogGroup {
    pub version: String,
    pub released_at: Option<String>,
    pub entries: Vec<String>,
}

/// Parse changelog entries from the embedded changelog string.
///
/// Current format per entry:
///   "hash<RS>tag<RS>timestamp<RS>subject"
/// where tag is either a version like "v0.4.2" or empty, timestamp is a
/// Unix epoch seconds string, and entries are separated by ASCII unit
/// separator (0x1F).
///
/// Older binaries used "hash:tag:subject"; we keep parsing that format too.
#[cfg(test)]
pub(super) fn parse_changelog_from(changelog: &str) -> Vec<ChangelogEntry<'_>> {
    parse_changelog_from_impl(changelog)
}

fn parse_changelog_from_impl(changelog: &str) -> Vec<ChangelogEntry<'_>> {
    if changelog.is_empty() {
        return Vec::new();
    }
    changelog
        .split('\x1f')
        .filter_map(|entry| {
            if entry.contains('\x1e') {
                let mut parts = entry.splitn(4, '\x1e');
                let hash = parts.next()?;
                let tag = parts.next().unwrap_or("");
                let timestamp = parts.next().and_then(|raw| raw.parse::<i64>().ok());
                let subject = parts.next()?;
                Some(ChangelogEntry {
                    hash,
                    tag,
                    timestamp,
                    subject,
                })
            } else {
                let (hash, rest) = entry.split_once(':')?;
                let (tag, subject) = rest.split_once(':')?;
                Some(ChangelogEntry {
                    hash,
                    tag,
                    timestamp: None,
                    subject,
                })
            }
        })
        .collect()
}

/// Parse the embedded changelog from the build-time environment.
fn parse_changelog() -> Vec<ChangelogEntry<'static>> {
    let changelog: &'static str = env!("JCODE_CHANGELOG");
    parse_changelog_from_impl(changelog)
}

fn format_changelog_timestamp(timestamp: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
}

#[cfg(test)]
pub(super) fn group_changelog_entries(
    entries: &[ChangelogEntry<'_>],
    current_version: &str,
    current_git_date: &str,
) -> Vec<ChangelogGroup> {
    group_changelog_entries_impl(entries, current_version, current_git_date)
}

fn group_changelog_entries_impl(
    entries: &[ChangelogEntry<'_>],
    current_version: &str,
    current_git_date: &str,
) -> Vec<ChangelogGroup> {
    if entries.is_empty() {
        return Vec::new();
    }

    let version_label = current_version
        .split_whitespace()
        .next()
        .unwrap_or(current_version);
    let unreleased_time =
        chrono::DateTime::parse_from_str(current_git_date, "%Y-%m-%d %H:%M:%S %z")
            .ok()
            .map(|dt| {
                dt.with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string()
            });

    let mut groups: Vec<ChangelogGroup> = Vec::new();
    let mut current_group = ChangelogGroup {
        version: format!("{} (unreleased)", version_label),
        released_at: unreleased_time,
        entries: Vec::new(),
    };

    for entry in entries {
        if !entry.tag.is_empty() {
            if !current_group.entries.is_empty() {
                groups.push(current_group);
            }
            current_group = ChangelogGroup {
                version: entry.tag.to_string(),
                released_at: entry.timestamp.and_then(format_changelog_timestamp),
                entries: vec![entry.subject.to_string()],
            };
        } else {
            current_group.entries.push(entry.subject.to_string());
        }
    }
    if !current_group.entries.is_empty() {
        groups.push(current_group);
    }

    groups
}

/// Return all embedded changelog entries grouped by release version.
/// Each group has a version label (e.g. "v0.4.2") and the commit subjects
/// that belong to that release. Commits before any tag are grouped under
/// the current build version.
pub(super) fn get_grouped_changelog() -> Vec<ChangelogGroup> {
    static GROUPS: OnceLock<Vec<ChangelogGroup>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            let entries = parse_changelog();
            group_changelog_entries_impl(&entries, env!("JCODE_VERSION"), env!("JCODE_GIT_DATE"))
        })
        .clone()
}

/// Get changelog entries the user hasn't seen yet.
/// Reads the last-seen commit hash from ~/.jcode/last_seen_changelog,
/// filters the embedded changelog to only new entries, then saves the latest hash.
/// Returns just the commit subjects (not the hashes).
pub(super) fn get_unseen_changelog_entries() -> &'static Vec<String> {
    static ENTRIES: OnceLock<Vec<String>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let all_entries = parse_changelog();
        if all_entries.is_empty() {
            return Vec::new();
        }

        let state_file = dirs::home_dir()
            .map(|h| h.join(".jcode").join("last_seen_changelog"))
            .unwrap_or_else(|| std::path::PathBuf::from(".jcode/last_seen_changelog"));

        let last_seen_hash = std::fs::read_to_string(&state_file)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let new_entries: Vec<String> = if last_seen_hash.is_empty() {
            all_entries
                .iter()
                .take(5)
                .map(|e| e.subject.to_string())
                .collect()
        } else {
            all_entries
                .iter()
                .take_while(|e| e.hash != last_seen_hash)
                .map(|e| e.subject.to_string())
                .collect()
        };

        if let Some(first) = all_entries.first() {
            if let Some(parent) = state_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&state_file, first.hash);
        }

        new_entries
    })
}
