use jcode_provider_core::{ModelMetadata, ModelRoute};
use std::collections::HashMap;
use std::path::PathBuf;

// The server catalog is authoritative for a remote UI. Scope it to this UI
// thread and home so neither other clients nor a different local profile can
// inherit the previous server's choices.
type RemoteChoices = (Option<PathBuf>, HashMap<String, ModelMetadata>);
thread_local! {
    static REMOTE_CHOICES: std::cell::RefCell<Option<RemoteChoices>> = const {
        std::cell::RefCell::new(None)
    };
}

pub(in crate::tui::app) fn register_remote_chatgpt_web_reasoning(routes: &[ModelRoute]) {
    let metadata = routes
        .iter()
        .filter(|route| route.api_method == "chatgpt-web")
        .filter_map(|route| {
            route
                .catalog_metadata
                .clone()
                .map(|metadata| (route.model.clone(), metadata))
        })
        .collect();
    REMOTE_CHOICES.with(|cache| {
        *cache.borrow_mut() = Some((crate::storage::jcode_dir().ok(), metadata));
    });
}

fn remote_metadata(model: &str) -> Option<ModelMetadata> {
    REMOTE_CHOICES.with(|cache| {
        let cache = cache.borrow();
        let (home, metadata) = cache.as_ref()?;
        if *home != crate::storage::jcode_dir().ok() {
            return None;
        }
        metadata.get(model).cloned()
    })
}

/// Selectable ChatGPT levels plus swarm modes. A fixed-level route has no
/// effort step. Legacy servers without route metadata use the local catalog.
pub(in crate::tui::app) fn chatgpt_web_reasoning_efforts(model: &str) -> Vec<&'static str> {
    let values: Vec<String> = match remote_metadata(model) {
        Some(metadata) => metadata.reasoning_efforts,
        None => crate::provider::chatgpt_web::cached_reasoning_options(model)
            .into_iter()
            .filter(|option| option.available)
            .map(|option| option.value)
            .collect(),
    };
    let mut efforts = Vec::new();
    for value in values {
        if let Some(effort) = jcode_provider_core::canonical_reasoning_effort(&value)
            && !efforts.contains(&effort)
        {
            efforts.push(effort);
        }
    }
    if efforts.len() < 2 {
        return Vec::new();
    }
    efforts.extend(["swarm", "swarm-deep"]);
    efforts
}

/// The ChatGPT name of a web route's level, including server-only choices.
pub(in crate::tui::app) fn chatgpt_web_effort_label(model: &str, effort: &str) -> Option<String> {
    if let Some(metadata) = remote_metadata(model) {
        return metadata.reasoning_effort_labels.get(effort).cloned();
    }
    crate::provider::chatgpt_web::cached_reasoning_options(model)
        .into_iter()
        .find(|option| option.value == effort)
        .map(|option| option.label)
}
