//! Differential oracle and paired benchmark for serving_route.
use super::*;
use crate::message::{Message, ToolDefinition};
use jcode_provider_core::EventStream;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct Probe {
    name: String,
    display: String,
    model: String,
    credential: Option<ResolvedCredential>,
    pin: Option<String>,
    routes: Vec<ModelRoute>,
    calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl Provider for Probe {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        anyhow::bail!("route probe must not serve requests")
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn display_name(&self) -> String {
        self.display.clone()
    }
    fn model(&self) -> String {
        self.model.clone()
    }
    fn active_resolved_credential(&self) -> Option<ResolvedCredential> {
        self.credential
    }
    fn explicit_provider_pin_for_current_model(&self) -> Option<String> {
        self.pin.clone()
    }
    fn model_routes(&self) -> Vec<ModelRoute> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.routes.clone()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

// Frozen pre-optimization implementation. Do not simplify: this is the oracle.
fn old(provider: &dyn Provider, selected_method: Option<&str>) -> Option<ModelRoute> {
    let model = provider.model();
    let display = provider.display_name();
    let provider_name = jcode_provider_core::normalize_model_route_provider_label(provider.name());
    let expected_method = match (
        provider_name.as_str(),
        provider.active_resolved_credential(),
    ) {
        ("claude" | "anthropic", Some(ResolvedCredential::ApiKey)) => Some("anthropic-api-key"),
        ("claude" | "anthropic", Some(ResolvedCredential::Oauth)) => Some("claude-oauth"),
        ("openai", Some(ResolvedCredential::ApiKey)) => Some("openai-api-key"),
        ("openai", Some(ResolvedCredential::Oauth)) => Some("openai-oauth"),
        ("copilot", _) => Some("copilot"),
        ("cursor", _) => Some("cursor"),
        ("bedrock", _) => Some("bedrock"),
        ("gemini", _) => Some("code-assist-oauth"),
        ("antigravity", _) => Some("antigravity-https"),
        _ => None,
    };
    let pin = provider.explicit_provider_pin_for_current_model();
    let mut candidates: Vec<_> = provider
        .model_routes()
        .into_iter()
        .filter(|route| {
            let same_model = route.model == model
                || (route.api_method_kind().is_openrouter()
                    && crate::provider::openrouter_catalog_model_id(&model).as_deref()
                        == Some(route.model.as_str()));
            let same_provider = if let Some(pin) = pin.as_ref() {
                route.api_method_kind().is_openrouter() && route.provider.eq_ignore_ascii_case(pin)
            } else {
                jcode_provider_core::model_route_provider_labels_match(&route.provider, &display)
            };
            same_model
                && same_provider
                && expected_method.is_none_or(|method| {
                    route.api_method_kind() == ModelRouteApiMethod::parse(method)
                })
        })
        .collect();
    if candidates.len() > 1
        && let Some(method) = selected_method
    {
        candidates.retain(|route| route.api_method_kind() == ModelRouteApiMethod::parse(method));
    }
    (candidates.len() == 1).then(|| candidates.remove(0))
}
fn row(model: &str, provider: &str, method: &str) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        provider: provider.into(),
        api_method: method.into(),
        available: false,
        detail: "preserve row metadata".into(),
        cheapness: None,
        usage: None,
    }
}
fn probe() -> Probe {
    Probe {
        name: "unknown".into(),
        display: "OpenRouter".into(),
        model: "gpt-5.4".into(),
        credential: None,
        pin: None,
        routes: vec![],
        calls: Arc::new(AtomicUsize::new(0)),
    }
}
fn parity(p: &Probe, selected: Option<&str>) {
    assert_eq!(
        serde_json::to_value(serving_route(p, selected)).unwrap(),
        serde_json::to_value(old(p, selected)).unwrap(),
        "name={} display={} model={} pin={:?} selected={selected:?}",
        p.name,
        p.display,
        p.model,
        p.pin
    );
}
#[test]
fn exact_parity_aliases_pins_models_credentials_and_ambiguity() {
    let labels = [
        "",
        " ",
        "Claude",
        "Anthropic",
        "Open AI",
        "Google",
        "Gemini",
        "Antigravity",
        "Copilot",
        "Copilot Code",
        "GitHub Copilot",
        "Cursor",
        "AWS_Bedrock",
        "Bedrock",
        "OpenRouter",
        "auto",
        "custom-Profile",
        "é",
    ];
    let methods = [
        "openrouter",
        "openai-oauth",
        "openai-api-key",
        "claude-oauth",
        "anthropic-api-key",
        "copilot",
        "cursor",
        "bedrock",
        "code-assist-oauth",
        "antigravity-https",
        "future:method",
        "",
    ];
    let mut p = probe();
    for display in labels {
        p.display = display.into();
        for label in labels {
            for method in methods {
                p.routes = vec![row(&p.model, label, method)];
                parity(&p, None);
            }
        }
    }
    for name in [
        "claude",
        "anthropic",
        "openai",
        "copilot",
        "cursor",
        "bedrock",
        "gemini",
        "antigravity",
        "unknown",
    ] {
        p.name = name.into();
        p.display = name.into();
        for credential in [
            None,
            Some(ResolvedCredential::ApiKey),
            Some(ResolvedCredential::Oauth),
        ] {
            p.credential = credential;
            for model in [
                "gpt-5.4",
                "claude-sonnet-4-6",
                "anthropic/claude-sonnet-4",
                "composer-2-fast",
                "",
                " gpt-5.4 ",
            ] {
                p.model = model.into();
                for pin in [None, Some("AUTO"), Some("OpenAI"), Some(""), Some(" auto ")] {
                    p.pin = pin.map(str::to_owned);
                    p.routes = methods
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            row(
                                if i % 2 == 0 { model } else { "openai/gpt-5.4" },
                                if i % 3 == 0 { "auto" } else { name },
                                m,
                            )
                        })
                        .collect();
                    for selected in [
                        None,
                        Some("openrouter"),
                        Some("openai-oauth"),
                        Some("future:method"),
                        Some(""),
                    ] {
                        parity(&p, selected);
                    }
                }
            }
        }
    }
    p = probe();
    p.routes = vec![row(&p.model, "auto", "openrouter")];
    assert!(
        serving_route(&p, Some("unrelated")).is_some(),
        "selected method must not filter a unique candidate"
    );
    p.routes.push(p.routes[0].clone());
    assert!(serving_route(&p, Some("openrouter")).is_none());
    p.routes.clear();
    assert!(serving_route(&p, None).is_none());
    p.routes.push(row(&p.model, "auto", "openrouter"));
    let before = p.calls.load(Ordering::Relaxed);
    assert!(serving_route(&p, None).is_some());
    p.routes.clear();
    assert!(serving_route(&p, None).is_none());
    assert_eq!(
        p.calls.load(Ordering::Relaxed) - before,
        2,
        "discovery stays live"
    );
}

#[test]
#[ignore = "paired timing benchmark, run explicitly with --profile selfdev --ignored --nocapture"]
fn paired_serving_route_benchmark() {
    use std::{hint::black_box, time::Instant};
    let mut p = probe();
    p.routes = (0..6000)
        .map(|i| {
            row(
                &format!("vendor-{}/model-{i}", i % 19),
                if i % 2 == 0 { "OpenRouter" } else { "auto" },
                "openrouter",
            )
        })
        .collect();
    p.routes.push(row("openai/gpt-5.4", "auto", "openrouter"));
    parity(&p, None);
    let iterations = 120;
    for _ in 0..10 {
        black_box(old(&p, None));
        black_box(serving_route(&p, None));
    }
    let measure = |f: fn(&dyn Provider, Option<&str>) -> Option<ModelRoute>| {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(f(black_box(&p), black_box(None)));
        }
        start.elapsed().as_secs_f64() * 1e6 / iterations as f64
    };
    for pair in 0..8 {
        let (old_us, new_us) = if pair % 2 == 0 {
            (measure(old), measure(serving_route))
        } else {
            let new = measure(serving_route);
            (measure(old), new)
        };
        println!(
            "pair={pair} routes={} old_us={old_us:.2} new_us={new_us:.2} speedup={:.3}",
            p.routes.len(),
            old_us / new_us
        );
    }
}
