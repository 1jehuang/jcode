//! User-created collections of sessions, stored in `~/.jcode/collections.json`.
//!
//! A collection is a named group the user builds by hand, independent of the
//! directory a session runs in. A session belongs to at most one collection.
//! Collections may carry a default working directory for new sessions and
//! instructions that clients attach to every turn of a member session.
//!
//! The store is a small JSON file shared by every client of one jcode home.
//! [`update`] serializes read-modify-write cycles with an advisory lock and
//! replaces the file atomically, so concurrent desktop windows never lose
//! each other's edits. These calls do blocking file IO.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest accepted collection name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest accepted instructions, in bytes.
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    /// Working directory for new sessions started inside this collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dir: Option<String>,
    /// Extra guidance attached to every turn of a member session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collections {
    #[serde(default)]
    pub version: u32,
    /// Display order.
    #[serde(default)]
    pub collections: Vec<Collection>,
    /// Session id to collection id.
    #[serde(default)]
    pub members: BTreeMap<String, String>,
}

impl Collections {
    pub fn get(&self, id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }

    /// The collection a session belongs to.
    pub fn collection_of(&self, session_id: &str) -> Option<&Collection> {
        self.get(self.members.get(session_id)?)
    }

    /// Instructions to attach to a session's turns, if any.
    pub fn instructions_for(&self, session_id: &str) -> Option<String> {
        let collection = self.collection_of(session_id)?;
        let text = collection.instructions.as_deref()?.trim();
        (!text.is_empty()).then(|| {
            format!(
                "This session is in the user's \"{}\" collection. Follow the collection's instructions:\n\n{}",
                collection.name, text
            )
        })
    }

    pub fn count(&self, id: &str) -> usize {
        self.members.values().filter(|c| *c == id).count()
    }

    /// Create a collection and return its id.
    pub fn create(
        &mut self,
        name: &str,
        default_dir: Option<String>,
        instructions: Option<String>,
    ) -> Result<String> {
        let id = self.new_id();
        self.create_with_id(&id, name, default_dir, instructions)?;
        Ok(id)
    }

    /// A fresh, unused collection id.
    pub fn new_id(&self) -> String {
        let created_at_ms = now_ms();
        let mut id = format!("collection_{created_at_ms}");
        let mut suffix = 1;
        while self.get(&id).is_some() {
            suffix += 1;
            id = format!("collection_{created_at_ms}_{suffix}");
        }
        id
    }

    /// Create a collection with a caller-chosen id. Clients that apply a
    /// change optimistically and then replay it against the file under
    /// [`update`] use this so both copies agree on the id.
    pub fn create_with_id(
        &mut self,
        id: &str,
        name: &str,
        default_dir: Option<String>,
        instructions: Option<String>,
    ) -> Result<()> {
        let name = validate_name(name)?;
        self.ensure_unique(&name, None)?;
        let id = id.trim();
        if id.is_empty() || self.get(id).is_some() {
            bail!("That collection id is already in use.");
        }
        self.collections.push(Collection {
            id: id.to_owned(),
            name,
            default_dir: clean_dir(default_dir),
            instructions: clean_instructions(instructions)?,
            created_at_ms: now_ms(),
        });
        Ok(())
    }

    /// Replace a collection's name, default directory, and instructions.
    pub fn edit(
        &mut self,
        id: &str,
        name: &str,
        default_dir: Option<String>,
        instructions: Option<String>,
    ) -> Result<()> {
        let name = validate_name(name)?;
        self.ensure_unique(&name, Some(id))?;
        let instructions = clean_instructions(instructions)?;
        let collection = self
            .collections
            .iter_mut()
            .find(|c| c.id == id)
            .context("That collection no longer exists.")?;
        collection.name = name;
        collection.default_dir = clean_dir(default_dir);
        collection.instructions = instructions;
        Ok(())
    }

    /// Delete a collection. Its sessions are kept and return to their
    /// directory groups.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.collections.len();
        self.collections.retain(|c| c.id != id);
        self.members.retain(|_, c| c != id);
        before != self.collections.len()
    }

    /// Put a session in a collection (moving it out of any other), or remove
    /// it from its collection with `None`.
    pub fn assign(&mut self, session_id: &str, collection: Option<&str>) -> Result<()> {
        match collection {
            Some(id) => {
                if self.get(id).is_none() {
                    bail!("That collection no longer exists.");
                }
                self.members.insert(session_id.to_owned(), id.to_owned());
            }
            None => {
                self.members.remove(session_id);
            }
        }
        Ok(())
    }

    /// Move a collection one step up (`-1`) or down (`1`) in display order.
    pub fn reorder(&mut self, id: &str, delta: isize) {
        let Some(index) = self.collections.iter().position(|c| c.id == id) else {
            return;
        };
        let target = index as isize + delta;
        if target >= 0 && (target as usize) < self.collections.len() {
            self.collections.swap(index, target as usize);
        }
    }

    fn ensure_unique(&self, name: &str, except: Option<&str>) -> Result<()> {
        if self
            .collections
            .iter()
            .any(|c| Some(c.id.as_str()) != except && c.name.eq_ignore_ascii_case(name))
        {
            bail!("A collection named \"{name}\" already exists.");
        }
        Ok(())
    }

    /// Drop memberships that point at missing collections.
    fn normalize(&mut self) {
        let ids = self
            .collections
            .iter()
            .map(|c| c.id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.members.retain(|_, c| ids.contains(c));
    }
}

fn validate_name(name: &str) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        bail!("Enter a collection name.");
    }
    if name.chars().count() > MAX_NAME_CHARS {
        bail!("Collection names are limited to {MAX_NAME_CHARS} characters.");
    }
    Ok(name)
}

fn clean_dir(dir: Option<String>) -> Option<String> {
    dir.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty())
}

fn clean_instructions(text: Option<String>) -> Result<Option<String>> {
    let text = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    if text
        .as_ref()
        .is_some_and(|t| t.len() > MAX_INSTRUCTIONS_BYTES)
    {
        bail!("Collection instructions are limited to 16 KiB.");
    }
    Ok(text)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// `collections.json` in the user's jcode home.
pub fn path() -> PathBuf {
    crate::launch::user_jcode_home().join("collections.json")
}

/// Load the store. A missing file is empty. A corrupt file is reported, so a
/// caller never silently overwrites the user's collections with nothing.
pub fn load_from(path: &Path) -> Result<Collections> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Collections::default());
        }
        Err(error) => return Err(error).context("Could not read collections"),
    };
    let mut store: Collections =
        serde_json::from_slice(&bytes).context("collections.json is not valid")?;
    store.normalize();
    Ok(store)
}

pub fn load() -> Result<Collections> {
    load_from(&path())
}

/// Read, modify, and atomically write the store under an exclusive lock.
pub fn update_at<T>(
    path: &Path,
    change: impl FnOnce(&mut Collections) -> Result<T>,
) -> Result<(Collections, T)> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Could not create the jcode home")?;
    }
    let _lock = lock(path)?;
    let mut store = load_from(path)?;
    let value = change(&mut store)?;
    store.version = 1;
    store.normalize();
    write(path, &store)?;
    Ok((store, value))
}

pub fn update<T>(change: impl FnOnce(&mut Collections) -> Result<T>) -> Result<(Collections, T)> {
    update_at(&path(), change)
}

fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("json.lock"))
        .context("Could not lock collections")?;
    file.lock().context("Could not lock collections")?;
    Ok(file)
}

fn write(path: &Path, store: &Collections) -> Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, store)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        anyhow::Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.context("Could not save collections")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assign_move_and_delete_keep_sessions() {
        let mut store = Collections::default();
        let research = store.create("  Q4   research ", None, None).unwrap();
        let client = store
            .create(
                "Client X",
                Some(" /work/x ".into()),
                Some(" Be brief. ".into()),
            )
            .unwrap();
        assert_ne!(research, client);
        assert_eq!(store.get(&research).unwrap().name, "Q4 research");
        assert_eq!(
            store.get(&client).unwrap().default_dir.as_deref(),
            Some("/work/x")
        );
        assert!(store.create("q4 RESEARCH", None, None).is_err());
        assert!(store.create("   ", None, None).is_err());

        store.assign("s1", Some(&research)).unwrap();
        store.assign("s2", Some(&research)).unwrap();
        assert_eq!(store.count(&research), 2);
        // Moving takes the session out of its previous collection.
        store.assign("s1", Some(&client)).unwrap();
        assert_eq!(store.count(&research), 1);
        assert_eq!(store.collection_of("s1").unwrap().id, client);
        assert!(
            store
                .instructions_for("s1")
                .unwrap()
                .contains("\"Client X\" collection")
        );
        assert!(store.instructions_for("s1").unwrap().ends_with("Be brief."));
        assert_eq!(store.instructions_for("s2"), None);
        assert!(store.assign("s3", Some("missing")).is_err());

        store.assign("s2", None).unwrap();
        assert_eq!(store.count(&research), 0);
        assert!(store.delete(&client));
        assert_eq!(store.collection_of("s1"), None);
        assert!(!store.delete(&client));

        let chosen = store.new_id();
        store.create_with_id(&chosen, "Chosen", None, None).unwrap();
        assert_eq!(store.get(&chosen).unwrap().name, "Chosen");
        assert!(store.create_with_id(&chosen, "Other", None, None).is_err());
        assert!(store.create_with_id(" ", "Blank", None, None).is_err());
    }

    #[test]
    fn edit_validates_names_and_reorder_stays_in_bounds() {
        let mut store = Collections::default();
        let a = store.create("A", None, None).unwrap();
        let b = store.create("B", None, None).unwrap();
        assert!(store.edit(&a, "b", None, None).is_err());
        store
            .edit(&a, "Alpha", Some("".into()), Some("".into()))
            .unwrap();
        let alpha = store.get(&a).unwrap();
        assert_eq!(alpha.name, "Alpha");
        assert_eq!(alpha.default_dir, None);
        assert_eq!(alpha.instructions, None);
        assert!(store.edit("gone", "Gone", None, None).is_err());
        assert!(
            store
                .edit(
                    &a,
                    "Alpha",
                    None,
                    Some("x".repeat(MAX_INSTRUCTIONS_BYTES + 1))
                )
                .is_err()
        );
        store.reorder(&b, -1);
        assert_eq!(store.collections[0].id, b);
        store.reorder(&b, -1);
        assert_eq!(store.collections[0].id, b);
        store.reorder(&a, 5);
        assert_eq!(store.collections[1].id, a);
    }

    #[test]
    fn store_round_trips_atomically_and_refuses_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/collections.json");
        assert_eq!(load_from(&path).unwrap(), Collections::default());
        let (_, id) = update_at(&path, |store| store.create("Notes", None, None)).unwrap();
        update_at(&path, |store| store.assign("s1", Some(&id))).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.collection_of("s1").unwrap().name, "Notes");
        // Failed changes leave the file untouched.
        assert!(update_at(&path, |store| store.create("notes", None, None)).is_err());
        assert_eq!(load_from(&path).unwrap(), loaded);
        // Dangling memberships are dropped on load.
        fs::write(
            &path,
            r#"{"collections":[],"members":{"s9":"collection_missing"}}"#,
        )
        .unwrap();
        assert!(load_from(&path).unwrap().members.is_empty());
        fs::write(&path, "{not json").unwrap();
        assert!(load_from(&path).is_err());
        assert!(update_at(&path, |store| store.create("X", None, None)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{not json");
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }
}
