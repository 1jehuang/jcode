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

fn ctx_route(model: &str, api: &str, detail: &str, cost: u64) -> ModelRoute {
    route(
        model,
        api,
        detail,
        Some(RouteBillingKind::Subscription),
        cost,
    )
}

fn with_tmp_state<T>(f: impl FnOnce() -> T) -> T {
    static L: Mutex<()> = Mutex::new(());
    let _g = L.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("bfr-{}-{:?}", std::process::id(), Instant::now()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("JCODE_ROUTER_STATE", dir.join("router-state.json")) };
    let out = f();
    unsafe { std::env::remove_var("JCODE_ROUTER_STATE") };
    let _ = std::fs::remove_dir_all(dir);
    out
}

#[test]
fn classifies_spawns_into_tiers() {
    let p = Policy::from_json(POLICY).unwrap();
    assert_eq!(
        p.classify_spawn("Implement the parser and add tests"),
        CODE_TIER
    );
    assert_eq!(
        p.classify_spawn("Read src/a.rs and src/b.rs, report findings"),
        LOCAL_TIER
    );
    assert_eq!(
        p.classify_spawn("Audit the whole repo for unsafe blocks"),
        SWARM_TIER
    );
    assert_eq!(
        p.classify_spawn("Read a.rs b.rs c.rs d.rs and summarize"),
        SWARM_TIER
    );
}

#[test]
fn true_local_rejects_cloud_and_proxy_tags() {
    let p = Policy::from_json(POLICY).unwrap();
    assert!(p.is_true_local(&route(
        "m",
        "openai-compatible:x",
        "(http://127.0.0.1:11434/v1)",
        None,
        0
    )));
    assert!(!p.is_true_local(&route(
        "m:cloud",
        "openai-compatible:x",
        "(http://localhost:11434/v1)",
        None,
        0
    )));
    assert!(!p.is_true_local(&route(
        "m",
        "openai-compatible:x",
        "(http://localhost:8080/v1) proxy",
        None,
        0
    )));
    assert!(!p.is_true_local(&route(
        "m",
        "openai-compatible:x",
        "(https://remote.example/v1)",
        None,
        0
    )));
}

#[test]
fn context_floor_and_payload_escalation() {
    let p = Policy::from_json(POLICY).unwrap();
    let r = vec![
        ctx_route("small", "x-oauth@a", "context ~70000 tokens", 50),
        ctx_route("big", "x-oauth@a", "context ~200000 tokens", 10),
    ];
    let code = p.rank(CODE_TIER, &r, &HashSet::new(), 0, &HashSet::new(), &|_| {
        true
    });
    assert_eq!(code.len(), 2);
    assert_eq!(
        p.rank(
            PARENT_TIER,
            &r,
            &HashSet::new(),
            0,
            &HashSet::new(),
            &|_| true
        )[0]
        .0
        .model,
        "big"
    );
    // 50k tokens * 1.5 = 75k > 70k: only the big window survives.
    let esc = p.rank(
        CODE_TIER,
        &r,
        &HashSet::new(),
        50_000,
        &HashSet::new(),
        &|_| true,
    );
    assert_eq!(
        esc.iter().map(|x| x.0.model.as_str()).collect::<Vec<_>>(),
        ["big"]
    );
}

#[test]
fn breaker_trips_only_on_listed_reasons_and_excludes_route() {
    with_tmp_state(|| {
        let p = Policy::from_json(POLICY).unwrap();
        let r = vec![
            ctx_route("a", "x-oauth@a", "context ~200000 tokens", 50),
            ctx_route("b", "x-oauth@a", "context ~200000 tokens", 5),
        ];
        assert!(!p.trip("a@x-oauth@a", "cap_exhausted"));
        assert!(p.trip("a@x-oauth@a", "429"));
        let st = with_state(|s| s.breakers.clone()).unwrap();
        let until = st["a@x-oauth@a"];
        assert!(until >= now_s() + 119 && until <= now_s() + 121);
        let d = dispatch(
            &p,
            "s-brk",
            "implement x",
            &r,
            &|_| true,
            std::process::id(),
        )
        .unwrap();
        assert_eq!((d.route.model.as_str(), d.tier), ("b", CODE_TIER));
    });
}

#[test]
fn local_slot_limit_spills_and_stale_pid_is_cleaned() {
    with_tmp_state(|| {
        let p = Policy::from_json(POLICY).unwrap();
        let r = vec![
            route(
                "loc",
                "openai-compatible:l",
                "(http://127.0.0.1:9/v1) context ~32000 tokens",
                None,
                0,
            ),
            ctx_route("shard", "x-oauth@a", "context ~200000 tokens", 1),
        ];
        // Another live process (pid 1) holds the only local slot -> spill to parallel_shard.
        with_state(|s| {
            s.local_slots
                .insert("loc@openai-compatible:l".into(), (1, now_s() + 600))
        });
        let d = dispatch(&p, "s-loc", "read a.rs", &r, &|_| true, std::process::id()).unwrap();
        assert_eq!(
            (d.requested_tier, d.tier, d.route.model.as_str()),
            (LOCAL_TIER, SWARM_TIER, "shard")
        );
        // A dead holder pid is reaped, so the local slot is ours.
        with_state(|s| {
            s.local_slots
                .insert("loc@openai-compatible:l".into(), (9_999_999, now_s() + 600))
        });
        let d = dispatch(&p, "s-loc2", "read a.rs", &r, &|_| true, std::process::id()).unwrap();
        assert_eq!((d.tier, d.billing), (LOCAL_TIER, Billing::Local));
    });
}

#[test]
fn metered_budget_steps_down_to_zero_marginal_cost() {
    with_tmp_state(|| {
        let p = Policy::from_json(POLICY).unwrap();
        let mut paid = route(
            "paid",
            "openai-compatible:paid",
            "(https://h/v1) context ~200000 tokens",
            None,
            1,
        );
        paid.cheapness = None;
        let r = vec![
            paid,
            ctx_route("sub", "x-oauth@a", "context ~200000 tokens", 9),
        ];
        let approved: HashSet<String> = ["paid".to_string()].into();
        // parallel_shard prefers metered_approved after local.
        assert_eq!(
            p.rank(SWARM_TIER, &r, &approved, 0, &HashSet::new(), &|_| true)[0]
                .0
                .model,
            "paid"
        );
        with_state(|s| s.metered_tokens.insert("s-bud".into(), u64::MAX / 2));
        // shortcut: dispatch reads approvals from the real config.toml, so assert the
        // budget filter directly on the over-budget allow-list.
        assert!(!p.over_budget_allowed(Billing::MeteredApproved));
        assert!(p.over_budget_allowed(Billing::Local) && p.over_budget_allowed(Billing::Oauth));
        let d = dispatch(
            &p,
            "s-bud",
            "audit everything",
            &r,
            &|_| true,
            std::process::id(),
        )
        .unwrap();
        assert!(d.over_budget && d.billing != Billing::MeteredApproved);
    });
}

#[test]
fn env_override_reads_tier_variable() {
    let p = Policy::from_json(POLICY).unwrap();
    unsafe { std::env::set_var("JCODE_ROUTE_TIER_MODEL_CODE_EXECUTION", "m-x") };
    assert_eq!(p.env_override(CODE_TIER).as_deref(), Some("m-x"));
    unsafe { std::env::remove_var("JCODE_ROUTE_TIER_MODEL_CODE_EXECUTION") };
    assert_eq!(p.env_override(CODE_TIER), None);
}
