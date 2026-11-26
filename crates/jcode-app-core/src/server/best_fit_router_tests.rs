use super::*;
use jcode_provider_core::{RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};

const POLICY: &str = include_str!("best_fit_router_fixture.json");

fn route(
    model: &str,
    api: &str,
    detail: &str,
    kind: Option<RouteBillingKind>,
    cost: u64,
) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        provider: "p".into(),
        api_method: api.into(),
        available: true,
        detail: detail.into(),
        cheapness: kind.map(|billing_kind| RouteCheapnessEstimate {
            billing_kind,
            source: RouteCostSource::PublicApiPricing,
            confidence: RouteCostConfidence::High,
            monthly_price_micros: None,
            input_price_per_mtok_micros: None,
            output_price_per_mtok_micros: None,
            cache_read_price_per_mtok_micros: None,
            included_requests_per_month: None,
            reference_input_tokens: 0,
            reference_output_tokens: 0,
            estimated_reference_cost_micros: Some(cost),
            note: None,
        }),
        usage: None,
    }
}

fn routes() -> Vec<ModelRoute> {
    vec![
        route("m-metered", "x-api", "", Some(RouteBillingKind::Metered), 9),
        route("m-web", "x-web", "", None, 0),
        route(
            "m-oauth-big",
            "x-oauth@a",
            "",
            Some(RouteBillingKind::Subscription),
            50,
        ),
        route(
            "m-oauth-small",
            "x-oauth@a",
            "",
            Some(RouteBillingKind::Subscription),
            5,
        ),
        route(
            "m-local",
            "openai-compatible:loc",
            "(http://localhost:1/v1)",
            None,
            0,
        ),
        route(
            "m-approved",
            "openai-compatible:paid",
            "(https://h/v1)",
            None,
            1,
        ),
        route("m-notools", "x-oauth@b", "no tools", None, 0),
    ]
}

fn approved() -> HashSet<String> {
    ["paid".to_string()].into()
}

#[test]
fn blocks_unapproved_metered_and_browser() {
    let p = Policy::from_json(POLICY).unwrap();
    let r = routes();
    assert_eq!(p.classify(&r[0], &approved()), Billing::Metered);
    assert!(!p.allowed(Billing::Metered));
    assert_eq!(p.classify(&r[1], &approved()), Billing::Browser);
    assert!(!p.allowed(Billing::Browser));
    assert_eq!(p.classify(&r[5], &HashSet::new()), Billing::Unknown);
    assert_eq!(p.classify(&r[5], &approved()), Billing::MeteredApproved);
    assert!(p.allowed(Billing::MeteredApproved));
}

#[test]
fn deep_tier_prefers_oauth_highest_reasoning_with_tools() {
    let p = Policy::from_json(POLICY).unwrap();
    let r = routes();
    let ranked = p.rank(PARENT_TIER, &r, &approved(), 0, &HashSet::new(), &|_| true);
    assert_eq!(ranked[0].0.model, "m-oauth-big");
    assert!(
        ranked
            .iter()
            .all(|(r, _)| r.model != "m-notools" && r.model != "m-metered" && r.model != "m-web")
    );
}

#[test]
fn shard_tier_conserves_quota_and_local_needs_probe() {
    let p = Policy::from_json(POLICY).unwrap();
    let r = routes();
    let warm = p.rank(SWARM_TIER, &r, &approved(), 0, &HashSet::new(), &|_| true);
    assert_eq!(warm[0].0.model, "m-local");
    assert_eq!(warm[1].0.model, "m-approved");
    let cold = p.rank(SWARM_TIER, &r, &approved(), 0, &HashSet::new(), &|_| false);
    assert_eq!(cold[0].0.model, "m-approved");
}

#[test]
fn sticky_pin_then_step_down() {
    let p = Policy::from_json(POLICY).unwrap();
    let r = routes();
    let (a, _, sticky_a) = pick(&p, "s-test", PARENT_TIER, &r, &|_| true).unwrap();
    let (b, _, sticky_b) = pick(&p, "s-test", PARENT_TIER, &r, &|_| true).unwrap();
    assert_eq!(
        (a.model.as_str(), sticky_a, sticky_b),
        ("m-oauth-big", false, true)
    );
    assert_eq!(b.model, a.model);
    assert!(!step_down(&p, "s-test", PARENT_TIER, "server_error"));
    assert!(step_down(
        &p,
        "s-test",
        PARENT_TIER,
        step_down_reason("HTTP 429 Too Many Requests").unwrap()
    ));
    let (c, _, sticky_c) = pick(&p, "s-test", PARENT_TIER, &r, &|_| true).unwrap();
    assert_eq!((c.model.as_str(), sticky_c), ("m-oauth-small", false));
}

#[test]
fn local_probe_rejects_closed_port_fast() {
    let r = route(
        "m",
        "openai-compatible:x",
        "(http://127.0.0.1:1/v1)",
        None,
        0,
    );
    let t = Instant::now();
    assert!(!local_probe(&r));
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[test]
fn metered_approval_reads_provider_toml() {
    let got = approved_from_toml(
        "[providers.Paid]\nmetered_approved = true\n[providers.other]\nbase_url = \"x\"\n",
    );
    assert_eq!(got, ["paid".to_string()].into());
}
