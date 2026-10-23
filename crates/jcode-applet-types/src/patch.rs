//! Incremental updates as RFC 6902 style operations on the JSON form of a
//! [`crate::Document`]. Paths address `/view/...`, `/state/...` and `/title`.
//!
//! Patches are applied to a copy and the result is revalidated, so a bad patch
//! never leaves a half-updated document on screen.
use crate::message::Document;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp {
    Add { path: String, value: Value },
    Replace { path: String, value: Value },
    Remove { path: String },
}

impl PatchOp {
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. } | Self::Replace { path, .. } | Self::Remove { path } => path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    RevisionMismatch { expected: u64, got: u64 },
    ForbiddenPath(String),
    PathNotFound(String),
    InvalidDocument(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionMismatch { expected, got } => {
                write!(f, "patch base revision {got} does not match {expected}")
            }
            Self::ForbiddenPath(path) => write!(f, "patch may not modify {path}"),
            Self::PathNotFound(path) => write!(f, "patch path not found: {path}"),
            Self::InvalidDocument(error) => write!(f, "patched document is invalid: {error}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Apply `ops` to `document` at `base_revision`. On success, returns the new
/// document with its revision incremented by one. On failure, `document` is
/// unchanged.
pub fn apply_patch(
    document: &Document,
    base_revision: u64,
    ops: &[PatchOp],
) -> Result<Document, PatchError> {
    if base_revision != document.revision {
        return Err(PatchError::RevisionMismatch {
            expected: document.revision,
            got: base_revision,
        });
    }
    let mut value =
        serde_json::to_value(document).map_err(|e| PatchError::InvalidDocument(e.to_string()))?;
    for op in ops {
        let path = op.path();
        let allowed = ["/view", "/state", "/title"]
            .iter()
            .any(|root| path == *root || path.starts_with(&format!("{root}/")));
        if !allowed {
            return Err(PatchError::ForbiddenPath(path.to_owned()));
        }
        apply_op(&mut value, op)?;
    }
    let mut next: Document =
        serde_json::from_value(value).map_err(|e| PatchError::InvalidDocument(e.to_string()))?;
    next.revision = document.revision + 1;
    Ok(next)
}

fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn apply_op(root: &mut Value, op: &PatchOp) -> Result<(), PatchError> {
    let path = op.path();
    let not_found = || PatchError::PathNotFound(path.to_owned());
    let (parent_path, last) = path.rsplit_once('/').ok_or_else(not_found)?;
    let last = unescape(last);
    let parent = if parent_path.is_empty() {
        Some(&mut *root)
    } else {
        root.pointer_mut(parent_path)
    }
    .ok_or_else(not_found)?;
    match (op, parent) {
        (PatchOp::Add { value, .. }, Value::Object(map)) => {
            map.insert(last, value.clone());
        }
        (PatchOp::Replace { value, .. }, Value::Object(map)) => {
            let slot = map.get_mut(&last).ok_or_else(not_found)?;
            *slot = value.clone();
        }
        (PatchOp::Remove { .. }, Value::Object(map)) => {
            map.remove(&last).ok_or_else(not_found)?;
        }
        (PatchOp::Add { value, .. }, Value::Array(items)) => {
            let index = if last == "-" {
                items.len()
            } else {
                last.parse::<usize>().map_err(|_| not_found())?
            };
            if index > items.len() {
                return Err(not_found());
            }
            items.insert(index, value.clone());
        }
        (PatchOp::Replace { value, .. }, Value::Array(items)) => {
            let index = last.parse::<usize>().map_err(|_| not_found())?;
            *items.get_mut(index).ok_or_else(not_found)? = value.clone();
        }
        (PatchOp::Remove { .. }, Value::Array(items)) => {
            let index = last.parse::<usize>().map_err(|_| not_found())?;
            if index >= items.len() {
                return Err(not_found());
            }
            items.remove(index);
        }
        _ => return Err(not_found()),
    }
    Ok(())
}
