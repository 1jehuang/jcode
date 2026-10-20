// A hand-edited `[pricing]` section takes effect on the next priced read (F18):
// no restart, and no waiting out a TTL. Two memos outside the config layer have
// to follow the edit, because neither key can see it on its own:
//
// * the route catalog's memo, which is what the model picker shows (its prices
//   are read from the config when the catalog is built), and
// * the TUI's per-model price memo, which holds what the derived layers last
//   resolved for the active model.
//
// Both now key on `model_pricing::pricing_generation()`, which changes exactly
// when the loaded config (and so the validated `[pricing]` view) does.

/// The model the route catalog prices: one the Anthropic route builder always
/// offers, and one a hand-written card can claim.
const CONFIG_EDITED_MODEL: &str = "claude-opus-4-6";

/// The model the TUI prices through its own memo. No rule claims it, so its
/// rates come from the derived layers and land in `cached_price_model`.
const MEMO_ONLY_MODEL: &str = "claude-haiku-4-5";

/// The config the test edits between priced reads. Its only price statement is
/// the card for [`CONFIG_EDITED_MODEL`], so the *only* thing that changes across
/// the edit is a number in `[pricing]`.
fn pricing_config_editing(model_input_price: &str) -> String {
    format!(
        r#"
[pricing.providers.claude.models."{CONFIG_EDITED_MODEL}".cost]
input = {model_input_price}
output = 2.0
"#
    )
}

/// What the model picker would show as the Anthropic API route's input rate.
///
/// Read through the provider's own route catalog, so it goes through the
/// route-catalog memo exactly like a rendered picker does.
fn claude_api_route_input_micros(provider: &crate::provider::MultiProvider) -> Option<u64> {
    use crate::provider::Provider as _;
    provider
        .model_routes()
        .into_iter()
        .find(|route| route.model == CONFIG_EDITED_MODEL && route.api_method == "claude-api")
        .and_then(|route| route.cheapness)
        .and_then(|cheapness| cheapness.input_price_per_mtok_micros)
}

/// Set `ANTHROPIC_API_KEY` for the duration of a test, restoring whatever was
/// there afterwards.
///
/// The `claude-api` route (the one whose price is config-authoritative) only
/// exists while an Anthropic API key is configured.
struct AnthropicApiKeyEnv(Option<String>);

impl AnthropicApiKeyEnv {
    fn set(value: &str) -> Self {
        let previous = std::env::var("ANTHROPIC_API_KEY").ok();
        crate::env::set_var("ANTHROPIC_API_KEY", value);
        crate::auth::AuthStatus::invalidate_cache();
        Self(previous)
    }
}

impl Drop for AnthropicApiKeyEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => crate::env::set_var("ANTHROPIC_API_KEY", value),
            None => crate::env::remove_var("ANTHROPIC_API_KEY"),
        }
        crate::auth::AuthStatus::invalidate_cache();
    }
}

/// A remote Anthropic session, which prices its calls through the TUI memo for
/// any model no hand-written rule claims.
fn remote_anthropic_app() -> App {
    let mut app = create_named_provider_test_app("anthropic", MEMO_ONLY_MODEL);
    app.is_remote = true;
    app.remote_provider_name = Some("Anthropic".to_string());
    app.remote_provider_model = Some(MEMO_ONLY_MODEL.to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    app
}

#[test]
fn config_change_triggers_repricing() {
    with_temp_jcode_home(|| {
        let _api_key = AnthropicApiKeyEnv::set("sk-ant-test-key");

        // 1. The user writes a `[pricing]` card, then opens a session. The TUI's
        //    memo for the memo-only model is warmed at this config: nothing
        //    changes it again until the card below is edited.
        write_pricing_config(&pricing_config_editing("1.0"));

        let mut app = remote_anthropic_app();
        app.begin_call_pricing(instant(0));
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(0));
        let memo_before = app
            .cost
            .cached_price_model
            .clone()
            .expect("the derived layers price the memo-only model");
        assert_eq!(
            app.cost.cached_prompt_price,
            Some(1.0),
            "the memo must hold what the derived layers resolved"
        );

        // Intentional ordering: building the provider (and the session above)
        // invalidates auth-derived pricing state, and an auth invalidation
        // makes the route memo stale for unrelated reasons. Warm the route
        // catalog *after* those, so what the next read observes is the
        // `[pricing]` edit and nothing else.
        let provider = crate::provider::MultiProvider::new_fast();
        assert_eq!(
            claude_api_route_input_micros(&provider),
            Some(1_000_000),
            "the picker route must start out priced from the written card"
        );

        // 2. The user hand-edits the card and the config reloads.
        write_pricing_config(&pricing_config_editing("9.0"));

        // 3. The picker re-prices: the route memo must not keep serving the
        //    catalog it built from the previous `[pricing]` section.
        assert_eq!(
            claude_api_route_input_micros(&provider),
            Some(9_000_000),
            "a `[pricing]` edit must re-price the picker without waiting out the memo TTL"
        );

        // 4. The TUI memo re-derives too: it must not still be serving the entry
        //    it resolved before the edit. A new call is what re-prices
        //    (`begin_call_pricing` drops the previous call's pinned card).
        app.begin_call_pricing(instant(0));
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(0));
        assert_ne!(
            app.cost.cached_price_model.as_deref(),
            Some(memo_before.as_str()),
            "a `[pricing]` edit must invalidate the TUI price memo"
        );
        assert_eq!(
            app.cost.cached_prompt_price,
            Some(1.0),
            "the memo must hold the rates the derived layers still resolve"
        );
    });
}
