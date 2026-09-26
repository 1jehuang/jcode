//! Persistence for agent-mounted applet instances, one file per session at
//! `~/.jcode/agent_applets/<session_id>.json`.
use anyhow::{Context, Result, anyhow, bail};
pub use jcode_applet_types::agent::{self, AgentApplets};
use jcode_applet_types::{
    Document, Instance, Limits, PatchOp, Placement, apply_patch, validate_document,
};
use std::path::PathBuf;
use std::sync::Mutex;

/// Maximum agent applet instances kept per session.
pub const MAX_INSTANCES_PER_SESSION: usize = 64;

static LOCK: Mutex<()> = Mutex::new(());

fn state_file(session_id: &str) -> Result<PathBuf> {
    if session_id.is_empty()
        || session_id.contains(['/', '\\'])
        || session_id.starts_with('.')
    {
        bail!("invalid session id for applets: {session_id:?}");
    }
    Ok(crate::storage::jcode_dir()?
        .join("agent_applets")
        .join(format!("{session_id}.json")))
}

fn load(session_id: &str) -> Result<AgentApplets> {
    let path = state_file(session_id)?;
    if !path.exists() {
        return Ok(AgentApplets::default());
    }
    crate::storage::read_json(&path)
}

fn save(session_id: &str, state: &AgentApplets) -> Result<()> {
    let path = state_file(session_id)?;
    if state.instances.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        return Ok(());
    }
    crate::storage::write_json_fast(&path, state)
}

fn validate(doc: &Document) -> Result<()> {
    validate_document(doc, &agent::manifest(), &Limits::default())
        .map_err(|e| anyhow!("invalid applet document: {e}"))
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("instance id must be 1-64 chars of [A-Za-z0-9-_.], got {id:?}");
    }
    Ok(())
}

/// Read, mutate, and persist a session's applets under one lock. On error,
/// nothing is written.
fn update<T>(
    session_id: &str,
    f: impl FnOnce(&mut AgentApplets) -> Result<T>,
) -> Result<(AgentApplets, T)> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut state = load(session_id)?;
    let out = f(&mut state)?;
    save(session_id, &state)?;
    Ok((state, out))
}

fn find<'a>(state: &'a mut AgentApplets, id: &str) -> Result<&'a mut Instance> {
    state
        .get_mut(id)
        .ok_or_else(|| anyhow!("no applet instance `{id}` in this session"))
}

pub fn snapshot_for_session(session_id: &str) -> Result<AgentApplets> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    load(session_id)
}

/// Insert or replace (by id, keeping position) an instance. The applet id is
/// forced to [`agent::APPLET_ID`].
pub fn mount(session_id: &str, mut instance: Instance) -> Result<AgentApplets> {
    validate_id(&instance.id)?;
    instance.applet = agent::APPLET_ID.to_string();
    validate(&instance.document)?;
    Ok(update(session_id, |state| {
        if let Some(slot) = state.get_mut(&instance.id) {
            // Revisions strictly increase so hosts can skip unchanged remounts.
            instance.document.revision = instance
                .document
                .revision
                .max(slot.document.revision + 1);
            *slot = instance;
        } else {
            if state.instances.len() >= MAX_INSTANCES_PER_SESSION {
                bail!(
                    "too many applet instances in this session (max {MAX_INSTANCES_PER_SESSION}); close some first"
                );
            }
            state.instances.push(instance);
        }
        Ok(())
    })?
    .0)
}

/// Apply patch ops. `base_revision` defaults to the current revision.
/// Returns the new snapshot and the new revision.
pub fn patch(
    session_id: &str,
    id: &str,
    base_revision: Option<u64>,
    ops: &[PatchOp],
) -> Result<(AgentApplets, u64)> {
    update(session_id, |state| {
        let inst = find(state, id)?;
        let base = base_revision.unwrap_or(inst.document.revision);
        let next = apply_patch(&inst.document, base, ops).map_err(|e| anyhow!("{e}"))?;
        validate(&next)?;
        let rev = next.revision;
        inst.document = next;
        Ok(rev)
    })
}

pub fn move_instance(session_id: &str, id: &str, placement: Placement) -> Result<AgentApplets> {
    Ok(update(session_id, |state| {
        let inst = find(state, id)?;
        inst.placement = placement;
        inst.document.revision += 1;
        Ok(())
    })?
    .0)
}

/// Remove an instance. Returns whether it existed.
pub fn close(session_id: &str, id: &str) -> Result<(AgentApplets, bool)> {
    update(session_id, |state| {
        let before = state.instances.len();
        state.instances.retain(|i| i.id != id);
        Ok(before != state.instances.len())
    })
}

/// Replace an instance's `document.state` (user input from the host). Does not
/// bump the revision, so agent patches based on it still apply.
pub fn set_state(
    session_id: &str,
    id: &str,
    value: serde_json::Value,
) -> Result<(AgentApplets, Instance)> {
    update(session_id, |state| {
        let inst = find(state, id)?;
        let mut doc = inst.document.clone();
        doc.state = if value.is_null() {
            serde_json::Value::Object(Default::default())
        } else {
            value
        };
        validate(&doc)?;
        inst.document = doc;
        Ok(inst.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_applet_types::{Anchor, Lifetime, Scope};
    use serde_json::json;

    fn inst(id: &str, title: &str) -> Instance {
        Instance {
            id: id.into(),
            applet: "whatever".into(),
            placement: Placement::Inline {
                session_id: "s".into(),
                anchor: Anchor::End,
            },
            scope: Scope::Session {
                session_id: "s".into(),
            },
            lifetime: Lifetime::Session,
            document: serde_json::from_value(json!({
                "revision": 1, "title": title,
                "view": {"type":"text","text":"hi"}
            }))
            .unwrap(),
        }
    }

    #[test]
    fn lifecycle() {
        let _env = crate::storage::lock_test_env();
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => crate::env::set_var("JCODE_HOME", v),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _home = Restore(prev);
        let sid = "sess_applets_test";
        assert!(snapshot_for_session(sid).unwrap().instances.is_empty());
        mount(sid, inst("a", "A")).unwrap();
        mount(sid, inst("b", "B")).unwrap();
        let snap = mount(sid, inst("a", "A2")).unwrap();
        assert_eq!(snap.instances[0].document.title, "A2");
        assert_eq!(snap.instances[0].document.revision, 2);
        assert_eq!(snap.instances[0].applet, agent::APPLET_ID);
        assert_eq!(snap.instances[1].id, "b");

        let bad: Vec<PatchOp> =
            serde_json::from_value(json!([{"op":"replace","path":"/state","value":[1]}]))
                .unwrap();
        assert!(patch(sid, "a", None, &bad).is_err());
        let ops: Vec<PatchOp> =
            serde_json::from_value(json!([{"op":"replace","path":"/title","value":"T"}])).unwrap();
        let (_, rev) = patch(sid, "a", None, &ops).unwrap();
        assert_eq!(rev, 3);
        assert!(patch(sid, "a", Some(1), &ops).is_err());

        let (_, i) = set_state(sid, "a", json!({"q":"x"})).unwrap();
        assert_eq!(i.document.state["q"], "x");
        assert_eq!(i.document.revision, 3, "state writes keep the revision");
        let snap = move_instance(sid, "a", Placement::Sidebar).unwrap();
        assert_eq!(snap.instances[0].document.revision, 4);
        assert!(set_state(sid, "a", json!([1])).is_err());
        assert!(mount(sid, inst("bad id!", "x")).is_err());

        let (snap, existed) = close(sid, "a").unwrap();
        assert!(existed);
        assert_eq!(snap.instances.len(), 1);
        assert_eq!(snapshot_for_session(sid).unwrap(), snap);
    }
}

/// Publish a session's snapshot to its connected clients.
pub fn publish(session_id: &str, snapshot: AgentApplets) {
    crate::bus::Bus::global().publish(crate::bus::BusEvent::AppletsUpdated(
        crate::bus::AppletsUpdated {
            session_id: session_id.to_string(),
            snapshot,
        },
    ));
}

fn short_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:04x}", h.finish() as u16)
}

/// Build the applet document an MCP resource block renders as, if any:
/// MCP-UI `ui://` HTML or URI lists, or native `application/vnd.jcode.applet+json`.
pub fn document_for_mcp_resource(
    uri: &str,
    mime_type: Option<&str>,
    text: Option<&str>,
) -> Option<Document> {
    let mime = mime_type.unwrap_or_default();
    let title = uri
        .strip_prefix("ui://")
        .unwrap_or(uri)
        .trim_end_matches('/')
        .to_string();
    let mut doc: Document = if mime == agent::JCODE_APPLET_MIME {
        serde_json::from_str(text?).ok()?
    } else if uri.starts_with("ui://") && mime.starts_with("text/uri-list") {
        let url = text?
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))?
            .to_string();
        serde_json::from_value(serde_json::json!({
            "revision": 1, "title": title,
            "view": {"type":"card","title": title,"children":[
                {"type":"text","text": url,"style":"caption","tone":"dim","max_lines":1},
                {"type":"button","label":"Open","variant":"primary",
                 "on_press":{"action":"host.open_url","args":{"url": url}}}
            ]}
        }))
        .ok()?
    } else if uri.starts_with("ui://") && (mime.starts_with("text/html") || text.is_some()) {
        serde_json::from_value(serde_json::json!({
            "revision": 1, "title": title,
            "view": {"type":"html","source": text?,"height":420}
        }))
        .ok()?
    } else {
        return None;
    };
    if doc.title.trim().is_empty() {
        doc.title = title;
    }
    Some(doc)
}

/// Mount an MCP resource as an inline applet anchored to `call_id`. Returns
/// the short replacement text for the tool output when it was mounted.
pub fn mount_mcp_resource(
    session_id: &str,
    call_id: &str,
    uri: &str,
    mime_type: Option<&str>,
    text: Option<&str>,
) -> Option<String> {
    if session_id.is_empty() {
        return None;
    }
    let document = document_for_mcp_resource(uri, mime_type, text)?;
    let id = format!("mcp-{}", short_hash(&format!("{call_id}\u{0}{uri}")));
    let instance = Instance {
        id,
        applet: agent::APPLET_ID.to_string(),
        placement: Placement::Inline {
            session_id: session_id.to_string(),
            anchor: jcode_applet_types::Anchor::ToolCall {
                call_id: call_id.to_string(),
            },
        },
        scope: jcode_applet_types::Scope::Session {
            session_id: session_id.to_string(),
        },
        lifetime: Default::default(),
        document,
    };
    match mount(session_id, instance) {
        Ok(snapshot) => {
            publish(session_id, snapshot);
            Some(format!("[Rendered UI: {uri}]"))
        }
        Err(error) => {
            crate::logging::warn(&format!("MCP UI resource {uri} not rendered: {error}"));
            None
        }
    }
}

#[cfg(test)]
mod mcp_tests {
    use super::*;

    #[test]
    fn mcp_resources_map_to_documents() {
        let html = document_for_mcp_resource("ui://w/chart", Some("text/html"), Some("<b>x</b>"))
            .unwrap();
        assert_eq!(serde_json::to_value(&html.view).unwrap()["type"], "html");
        validate(&html).unwrap();
        let link = document_for_mcp_resource(
            "ui://w/link",
            Some("text/uri-list"),
            Some("# c\nhttps://example.com\n"),
        )
        .unwrap();
        validate(&link).unwrap();
        assert!(serde_json::to_string(&link).unwrap().contains("host.open_url"));
        let native = document_for_mcp_resource(
            "file:///x",
            Some(agent::JCODE_APPLET_MIME),
            Some(r#"{"revision":1,"title":"N","view":{"type":"text","text":"hi"}}"#),
        )
        .unwrap();
        assert_eq!(native.title, "N");
        assert!(document_for_mcp_resource("file:///x", Some("text/html"), Some("x")).is_none());
    }
}
