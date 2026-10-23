use super::{MERMAID_SOURCE_BY_HASH, RenderProfile, current_render_profile};

pub fn current_preferred_aspect_ratio_bucket() -> Option<u16> {
    current_render_profile().preferred_aspect_per_mille
}

pub fn preferred_aspect_ratio_bucket(ratio: Option<f32>) -> Option<u16> {
    RenderProfile::from_preferred_aspect_ratio(ratio).preferred_aspect_per_mille
}

pub fn mermaid_source_for_hash(hash: u64) -> Option<String> {
    MERMAID_SOURCE_BY_HASH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&hash)
        .cloned()
}
