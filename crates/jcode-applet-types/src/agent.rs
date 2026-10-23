//! The built-in agent applet: instances mounted by the agent's `applet` tool
//! (and by the MCP-UI bridge) inside one session.
use crate::manifest::{Capability, Manifest};
use crate::message::Instance;
use serde::{Deserialize, Serialize};

/// Applet id of every agent-mounted instance.
pub const APPLET_ID: &str = "jcode.agent";

/// MIME type of an MCP resource whose text is a native applet [`crate::Document`].
pub const JCODE_APPLET_MIME: &str = "application/vnd.jcode.applet+json";

/// Manifest shared by every agent-mounted instance.
pub fn manifest() -> Manifest {
    Manifest {
        schema: crate::SCHEMA.to_string(),
        id: APPLET_ID.to_string(),
        title: "Agent".to_string(),
        icon: None,
        description: None,
        launchers: Vec::new(),
        tool_cards: Vec::new(),
        capabilities: vec![
            Capability::OpenUrl,
            Capability::Clipboard,
            Capability::SendPrompt,
            Capability::StartChat,
            Capability::Html,
        ],
    }
}

/// The complete set of agent-mounted instances for one session, in mount
/// order. Instance ids are short and session-local; hosts namespace them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentApplets {
    #[serde(default)]
    pub instances: Vec<Instance>,
}

impl AgentApplets {
    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Limits, validate_document, validate_manifest};

    #[test]
    fn manifest_is_valid_and_allows_html() {
        let m = manifest();
        validate_manifest(&m).unwrap();
        assert!(m.allows(Capability::Html));
        assert!(!m.allows(Capability::ReadFiles));
    }

    #[test]
    fn snapshot_round_trips_and_defaults_empty() {
        let empty: AgentApplets = serde_json::from_str("{}").unwrap();
        assert!(empty.instances.is_empty());
        let json = serde_json::json!({"instances":[{
            "id":"chart-3f2a","applet":APPLET_ID,
            "placement":{"kind":"inline","session_id":"s","anchor":{"kind":"end"}},
            "scope":{"kind":"session","session_id":"s"},
            "document":{"revision":1,"title":"Chart","view":{"type":"button","label":"Go","on_press":{"action":"host.open_url","args":{"url":"https://x.y"}}}}
        }]});
        let snap: AgentApplets = serde_json::from_value(json).unwrap();
        let doc = &snap.get("chart-3f2a").unwrap().document;
        validate_document(doc, &manifest(), &Limits::default()).unwrap();
        let back: AgentApplets =
            serde_json::from_value(serde_json::to_value(&snap).unwrap()).unwrap();
        assert_eq!(back, snap);
    }
}
