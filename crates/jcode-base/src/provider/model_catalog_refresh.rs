//! Account-scoped catalog refresh admission, retry throttling, and completion.
use super::*;

pub fn note_openai_model_catalog_refresh_attempt() {
    OPENAI_MODEL_CATALOG_SERVICE.note_attempt(&current_openai_account_scope());
}

pub(super) fn note_openai_model_catalog_refresh_attempt_for_scope(scope: &str) {
    OPENAI_MODEL_CATALOG_SERVICE.note_attempt(scope);
}

fn anthropic_model_catalog_refresh_throttled(scope: &str) -> bool {
    ANTHROPIC_MODEL_CATALOG_SERVICE.refresh_throttled(scope)
}

pub fn should_refresh_openai_model_catalog() -> bool {
    should_refresh_openai_model_catalog_for_scope(&current_openai_account_scope())
}

pub fn should_refresh_openai_model_catalog_for_scope(scope: &str) -> bool {
    if OPENAI_MODEL_CATALOG_SERVICE.is_fresh(scope) {
        return false;
    }
    if OPENAI_MODEL_CATALOG_SERVICE.refresh_throttled(scope) {
        return false;
    }
    OPENAI_MODEL_CATALOG_SERVICE.should_refresh(scope)
}

pub fn should_refresh_anthropic_model_catalog() -> bool {
    let scope = current_anthropic_catalog_scope();
    should_refresh_anthropic_model_catalog_for_scope(&scope)
}

pub fn should_refresh_anthropic_model_catalog_for_scope(scope: &str) -> bool {
    // Hydration is the purpose of this read. A missing snapshot leaves the
    // service empty, so the freshness/admission checks below request a fetch.
    let _cached_models = cached_anthropic_model_ids_for_scope(scope);
    if anthropic_model_cache_is_fresh(scope) {
        return false;
    }
    if anthropic_model_catalog_refresh_throttled(scope) {
        return false;
    }
    ANTHROPIC_MODEL_CATALOG_SERVICE.should_refresh(scope)
}

pub fn begin_openai_model_catalog_refresh() -> bool {
    begin_openai_model_catalog_refresh_for_scope(&current_openai_account_scope())
}

pub fn begin_openai_model_catalog_refresh_for_scope(scope: &str) -> bool {
    OPENAI_MODEL_CATALOG_SERVICE.begin_refresh(scope)
}

pub fn begin_anthropic_model_catalog_refresh() -> Option<String> {
    let scope = current_anthropic_catalog_scope();
    begin_anthropic_model_catalog_refresh_for_scope(&scope).then_some(scope)
}

pub fn begin_anthropic_model_catalog_refresh_for_scope(scope: &str) -> bool {
    // Hydrate before checking TTL so a fresh disk snapshot avoids a request,
    // while a stale snapshot remains visible during background revalidation.
    // Hydration is the purpose of this read. A missing snapshot leaves the
    // service empty, so the freshness/admission checks below request a fetch.
    let _cached_models = cached_anthropic_model_ids_for_scope(scope);
    ANTHROPIC_MODEL_CATALOG_SERVICE.begin_refresh(scope)
}

pub fn begin_forced_anthropic_model_catalog_refresh_for_scope(scope: &str) -> bool {
    ANTHROPIC_MODEL_CATALOG_SERVICE.begin_forced_refresh(scope)
}

pub fn finish_openai_model_catalog_refresh() {
    OPENAI_MODEL_CATALOG_SERVICE.finish_refresh(&current_openai_account_scope());
}

pub fn finish_openai_model_catalog_refresh_for_scope(scope: &str) {
    OPENAI_MODEL_CATALOG_SERVICE.finish_refresh(scope);
}

pub fn finish_anthropic_model_catalog_refresh_for_scope(scope: &str) {
    ANTHROPIC_MODEL_CATALOG_SERVICE.finish_refresh(scope);
}

pub(super) fn anthropic_model_cache_is_fresh(scope: &str) -> bool {
    ANTHROPIC_MODEL_CATALOG_SERVICE.is_fresh(scope)
}
