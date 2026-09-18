// Pricing one API call: the rate card is resolved at the call's own instant
// (F15), pinned to that call (F16), and a configured card that cannot price the
// call is never replaced by the generic $15/$60 estimate (spec 4.4).

/// DeepSeek's published peak windows as a user would write them: peak is UTC
/// 01:00-04:00 on weekdays, everything else is off-peak. Peak is 10x off-peak
/// so a leaked tier is impossible to miss.
const PEAK_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
default_tariff = "off_peak"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.0
output = 2.0

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.off_peak]
input = 1.0
output = 2.0

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.peak]
multiplier = 10.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"]]
"#;

/// A card denominated in CNY with no `output` rate, and nothing underneath it:
/// `sources::resolve_card` cannot complete it (a CNY card must not inherit USD
/// numbers), so it can only price input tokens.
const HALF_WRITTEN_CNY_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
"#;

/// A fixed instant, so nothing here depends on the wall clock.
fn instant(epoch_secs: u64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch_secs)
}

/// 2026-09-14T00:59:59Z (Monday): one second before the peak window opens.
const ONE_SECOND_BEFORE_PEAK: u64 = 1_789_347_599;
/// 2026-09-14T01:30:00Z (Monday): inside the peak window.
const INSIDE_PEAK: u64 = 1_789_349_400;
/// 2030-06-24T02:00:00Z (Monday): inside the peak window, four years away from
/// the wall clock so "the pinned instant decided" cannot be a coincidence.
const FAR_FUTURE_PEAK: u64 = 1_908_496_800;
/// 2030-06-22T02:00:00Z (Saturday): the same clock reading on a weekend, which
/// the schedule puts off-peak.
const FAR_FUTURE_OFF_PEAK: u64 = 1_908_324_000;

/// Write `toml` as the user's `config.toml` in the isolated home the test is
/// already running in, and drop any config loaded before it.
fn write_pricing_config(toml: &str) {
    let home = std::env::var_os("JCODE_HOME").expect("test home is set");
    std::fs::write(std::path::PathBuf::from(home).join("config.toml"), toml)
        .expect("write config.toml");
    crate::config::invalidate_config_cache();
}

/// A remote DeepSeek session: billed per token, priced from the user's config.
fn remote_deepseek_app() -> App {
    let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
    app.is_remote = true;
    app.remote_provider_name = Some("DeepSeek".to_string());
    app.remote_provider_model = Some("deepseek-v4-pro".to_string());
    app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);
    app
}

#[test]
fn same_call_keeps_tier_across_boundary() {
    // F15/F16: a call is priced once, at the instant of its first usage
    // snapshot. A later delta snapshot that lands inside the peak window must
    // still bill the tier the call started in, otherwise a long call that
    // straddles the boundary is re-priced mid-flight.
    with_temp_jcode_home(|| {
        write_pricing_config(PEAK_CARD_CONFIG);
        let mut app = remote_deepseek_app();

        // First snapshot: 00:59:59Z, one second before the peak window opens.
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(ONE_SECOND_BEFORE_PEAK));
        assert!(
            (session_cost_usd(&app) - 1.0).abs() < 1e-4,
            "off-peak input is $1.00/Mtok, got ${:.4}",
            session_cost_usd(&app)
        );

        // Delta snapshot inside the peak window: still the same call, so still
        // the off-peak card. Peak output would be $20.00/Mtok.
        app.accrue_remote_call_cost(0, 1_000_000, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - 3.0).abs() < 1e-4,
            "the whole call bills off-peak ($1.00 + $2.00), got ${:.4}",
            session_cost_usd(&app)
        );

        // A *new* call at the peak instant really does bill peak rates, so the
        // assertion above pins the tier rather than proving the schedule inert.
        app.begin_api_call_accounting();
        let before = session_cost_usd(&app);
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - before - 10.0).abs() < 1e-4,
            "a call started inside the peak window bills 10x, got ${:.4}",
            session_cost_usd(&app) - before
        );
    });
}

#[test]
fn incomplete_config_card_is_not_silently_priced_at_generic_defaults() {
    // Spec 4.4: when the user configured a price for this model but the card
    // cannot price the call, refuse to price it. Falling back to $15/$60 would
    // report a number the user never wrote (and overstate it by ~2 orders of
    // magnitude for a CNY card).
    with_temp_jcode_home(|| {
        write_pricing_config(HALF_WRITTEN_CNY_CARD_CONFIG);
        let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
        app.streaming.streaming_input_tokens = 1_000_000;
        app.streaming.streaming_output_tokens = 1_000_000;

        app.update_cost_impl();

        assert!(
            session_cost_usd(&app).abs() < 1e-9,
            "an incomplete configured card must not be topped up with $15/$60 defaults"
        );
    });
}

#[test]
fn local_path_uses_call_time() {
    // F15 for the local path: a turn is billed with the tariff in effect when
    // its request was *sent*, not the wall clock read when the cost is
    // computed. Both instants are years away from now and on opposite sides of
    // the schedule, so resolving at `SystemTime::now()` cannot satisfy both.
    with_temp_jcode_home(|| {
        write_pricing_config(PEAK_CARD_CONFIG);

        let local_turn_cost = |at_secs: u64| {
            let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
            app.streaming.streaming_input_tokens = 1_000_000;
            app.streaming.streaming_output_tokens = 1_000_000;
            app.begin_call_pricing(instant(at_secs));
            app.update_cost_impl();
            session_cost_usd(&app)
        };

        // Monday 02:00Z, inside the peak window: 1M in at $10 + 1M out at $20.
        let peak_cost = local_turn_cost(FAR_FUTURE_PEAK);
        assert!(
            (peak_cost - 30.0).abs() < 1e-4,
            "a turn sent inside the peak window bills 10x, got ${peak_cost:.4}"
        );

        // Saturday 02:00Z, the same reading on a weekend: the base card.
        let off_peak_cost = local_turn_cost(FAR_FUTURE_OFF_PEAK);
        assert!(
            (off_peak_cost - 3.0).abs() < 1e-4,
            "a turn sent off-peak bills the base card ($1 + $2), got ${off_peak_cost:.4}"
        );
    });
}

#[test]
fn unconfigured_model_keeps_the_generic_default_fallback() {
    // Global constraint 1: with no `[pricing]` section at all, the pre-feature
    // behaviour stands, including the generic per-token estimate for a model
    // nothing can price.
    with_temp_jcode_home(|| {
        let mut app = create_named_provider_test_app("deepseek", "deepseek-unknown-model");
        app.streaming.streaming_input_tokens = 1_000_000;
        app.streaming.streaming_output_tokens = 1_000_000;

        app.update_cost_impl();

        assert!(
            (session_cost_usd(&app) - 75.0).abs() < 1e-4,
            "unconfigured unknown models keep the $15/$60 estimate, got ${:.4}",
            session_cost_usd(&app)
        );
    });
}

/// A complete CNY card: unlike `HALF_WRITTEN_CNY_CARD_CONFIG` this one can
/// price a whole call, so the billing path really does produce a CNY amount.
const COMPLETE_CNY_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 7.0
output = 8.0
"#;

#[test]
fn non_usd_spend_lands_in_its_own_ledger_bucket() {
    // The activity ledger used to be USD-only, so the billing path skipped it
    // for any other currency. With per-currency buckets that skip is gone: a
    // CNY call must be recorded as CNY, not dropped and not relabelled.
    with_temp_jcode_home(|| {
        write_pricing_config(COMPLETE_CNY_CARD_CONFIG);
        let mut app = remote_deepseek_app();
        app.streaming.streaming_input_tokens = 1_000_000;

        app.update_cost_impl();
        let cny = Currency::new("cny");
        assert!(
            (app.cost.total_in(&cny) - 7.0).abs() < 1e-4,
            "1M input tokens on a CNY 7.0/Mtok card is CNY 7.00, got {}",
            app.cost.total_in(&cny)
        );
        assert!(
            app.cost
                .total_cost_by_currency
                .get(&Currency::usd())
                .is_none(),
            "a CNY call must not be filed under USD: {:?}",
            app.cost.total_cost_by_currency
        );

        // The ledger write is spawned off the render loop, so wait for the file
        // instead of assuming it is already there.
        let path = std::path::PathBuf::from(std::env::var_os("JCODE_HOME").expect("test home"))
            .join("provider_activity.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let recorded = loop {
            if let Ok(raw) = std::fs::read_to_string(&path)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw)
                && let Some(entries) = json["entries"].as_object()
                && let Some(spend) = entries
                    .values()
                    .map(|entry| &entry["spend"])
                    .find(|spend| spend["day"]["CNY"] == serde_json::json!(7.0))
            {
                break spend.clone();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no CNY bucket appeared in the ledger within 10s"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(
            recorded["day_usd"], 7.0,
            "the USD mirror tracks the single CNY bucket"
        );
    });
}

#[test]
fn session_total_keeps_one_bucket_per_currency() {
    // F21: a session that bills in two currencies keeps two buckets. The total
    // is never one scalar labelled with whichever call was priced last.
    with_temp_jcode_home(|| {
        let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
        app.streaming.streaming_input_tokens = 1_000_000;

        write_pricing_config(COMPLETE_CNY_CARD_CONFIG);
        app.update_cost_impl();
        assert!((app.cost.total_in(&Currency::new("CNY")) - 7.0).abs() < 1e-4);

        // Same session, now priced by the USD card (the default when no
        // `currency` is written). The instant is pinned off-peak: this test is
        // about currency buckets, and reading the wall clock made it fail
        // whenever the run happened to land inside the configured peak window
        // (10x input, so the USD bucket read 10.0 instead of 1.0).
        write_pricing_config(PEAK_CARD_CONFIG);
        app.streaming.streaming_input_tokens = 1_000_000;
        app.streaming.streaming_output_tokens = 0;
        app.begin_api_call_accounting_at(instant(FAR_FUTURE_OFF_PEAK));
        app.update_cost_impl();

        assert!(
            (app.cost.total_in(&Currency::usd()) - 1.0).abs() < 1e-4,
            "the USD call lands in the USD bucket: {:?}",
            app.cost.total_cost_by_currency
        );
        assert!(
            (app.cost.total_in(&Currency::new("CNY")) - 7.0).abs() < 1e-4,
            "the earlier CNY spend is untouched by the USD call: {:?}",
            app.cost.total_cost_by_currency
        );
    });
}

// F8/F20: a `[pricing]` rule that is out of effect falls back to the next layer
// when `on_rule_expiry = "fallback"` (the default). Falling back silently is
// exactly the silent-wrong-price class this feature exists to remove, so the
// display has to say the user's own rule stopped applying.

/// A rule whose validity window ended in 2020, with the default
/// `on_rule_expiry = "fallback"` (the field is not written on purpose).
const EXPIRED_RULE_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 7.0
output = 8.0
"#;

/// The same rule, but refusing to price the call once it is out of effect.
const EXPIRED_RULE_NO_PRICE_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"
on_rule_expiry = "no_price"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 7.0
output = 8.0
"#;

/// A rule that only starts in 2030, so it is not in effect yet.
const FUTURE_RULE_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_from = "2030-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 7.0
output = 8.0
"#;

/// Bill one remote call on the active model and return the session cost plus
/// the cost line the info widget shows for it, i.e. what the user reads.
fn widget_cost_line() -> (f32, String) {
    let mut app = remote_deepseek_app();
    app.accrue_remote_call_cost(1_000_000, 1_000_000, 0, 0, std::time::SystemTime::now());
    let data = crate::tui::TuiState::info_widget_data(&app);
    let usage = data
        .usage_info
        .as_ref()
        .expect("a cost-based provider shows a cost widget");
    (
        session_cost_usd(&app),
        crate::money_display::summarize(&usage.cost_rows, 4),
    )
}

#[test]
fn expired_rule_is_labelled_where_the_user_reads_the_price() {
    with_temp_jcode_home(|| {
        // No config at all: the number the fallback layer produces for this
        // model, which the expired rule must fall back to unchanged.
        let (fallback_cost, unlabelled) = widget_cost_line();

        write_pricing_config(EXPIRED_RULE_CONFIG);
        let (expired_cost, labelled) = widget_cost_line();

        assert!(
            (expired_cost - fallback_cost).abs() < 1e-4,
            "an expired rule with on_rule_expiry = fallback prices like the next layer: \
             {expired_cost} vs {fallback_cost}"
        );
        assert!(
            !unlabelled.contains("expired"),
            "sanity: nothing is labelled without a rule: {unlabelled}"
        );
        assert!(
            labelled.contains("(rule expired)"),
            "the widget must say the hand-written rule stopped applying: {labelled}"
        );
    });
}

#[test]
fn a_rule_that_only_starts_later_is_labelled_as_not_in_effect() {
    with_temp_jcode_home(|| {
        write_pricing_config(FUTURE_RULE_CONFIG);
        let (cost, line) = widget_cost_line();

        assert!(cost > 0.0, "the fallback layer prices the call: {line}");
        assert!(
            line.contains("(rule not in effect yet)"),
            "a rule whose window has not opened yet is not 'expired': {line}"
        );
    });
}

#[test]
fn a_rule_that_refuses_to_price_is_never_labelled_expired_but_priced() {
    // `on_rule_expiry = "no_price"` bills nothing, so the display must not
    // carry a "the rule expired, this is the fallback price" marker: there is
    // no fallback price to explain. What it *must* carry is the honest label
    // for why the figure is what it is: the user's own rule refused to price the
    // call, so the zero on screen is not a price and must not read like one
    // (F-C, spec 4.4).
    with_temp_jcode_home(|| {
        write_pricing_config(EXPIRED_RULE_NO_PRICE_CONFIG);
        let (cost, line) = widget_cost_line();

        assert_eq!(cost, 0.0, "no_price refuses to price the call: {line}");
        assert!(
            !line.contains("expired"),
            "an unpriced call must not be labelled as an expired rule that was priced: {line}"
        );
        assert!(
            line.contains("rule cannot price this call"),
            "the cost line must say the rule refused to price the call instead of \
             showing a bare $0.0000 that reads as 'free': {line}"
        );
    });
}

// I-1: a configured `cache_write` rate has to be the rate the call is billed
// at. The billing site used to apply Anthropic's `input x 1.25/2.0` cache-write
// premium for every card, so the value the user wrote (and the config layer
// validated and carried) was silently replaced by a heuristic.

/// A card that states the cache-write rate, so the heuristic cannot be the
/// answer: 0.9 is neither `3.0 x 1.25` nor `3.0 x 2.0`.
fn cache_write_card_config() -> String {
    format!(
        r#"
[pricing.providers."claude:api-key".models."{MEMO_ONLY_MODEL}".cost]
input = 3.0
output = 15.0
cache_write = 0.9
"#
    )
}

/// The premium the billing heuristic applies to the input rate when the user
/// configured no cache-write rate (Anthropic: 1.25x for the 5-minute TTL, 2x
/// for the 1-hour one).
fn anthropic_cache_write_multiplier() -> f32 {
    if crate::provider::anthropic::is_cache_ttl_1h() {
        2.0
    } else {
        1.25
    }
}

#[test]
fn a_configured_cache_write_rate_is_honoured_instead_of_the_heuristic() {
    with_temp_jcode_home(|| {
        write_pricing_config(&cache_write_card_config());
        let mut app = remote_anthropic_app();
        // 1M fresh input plus 1M cache creation: cache writes dominate the call,
        // so the configured rate and the heuristic cannot be confused.
        app.accrue_remote_call_cost(1_000_000, 0, 0, 1_000_000, std::time::SystemTime::now());

        let billed = session_cost_usd(&app);
        let expected = 3.0 + 0.9;
        let heuristic = 3.0 + 3.0 * anthropic_cache_write_multiplier();
        assert!(
            (billed - expected).abs() < 1e-4,
            "a configured cache_write must be billed: expected ${expected:.4}, got ${billed:.4} \
             (the input x{} heuristic would be ${heuristic:.4})",
            anthropic_cache_write_multiplier()
        );
    });
}

#[test]
fn an_unconfigured_cache_write_keeps_the_anthropic_split_accounting_premium() {
    // Parity constraint: with no `[pricing]` rule for this model the pre-feature
    // behaviour stands, so cache creation costs the input rate times the
    // Anthropic premium and nothing else.
    with_temp_jcode_home(|| {
        let mut app = remote_anthropic_app();
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, std::time::SystemTime::now());
        let input_only = session_cost_usd(&app);
        assert!(
            input_only > 0.0,
            "the fallback layer prices the input tokens"
        );

        app.begin_api_call_accounting();
        app.accrue_remote_call_cost(0, 0, 0, 1_000_000, std::time::SystemTime::now());
        let creation = session_cost_usd(&app) - input_only;

        let multiplier = anthropic_cache_write_multiplier();
        let expected = input_only * multiplier;
        assert!(
            (creation - expected).abs() < 1e-4,
            "an unconfigured cache write bills at {multiplier}x the input rate: \
             expected ${expected:.4}, got ${creation:.4}"
        );
    });
}

// I-2: a `[pricing]` section that fails validation is dropped whole, so every
// hand-written rule in it stops applying and prices revert to models.dev. That
// is the same silent-wrong-price class as an expired rule, and it has to be
// visible where the amount is read.

/// A section with one invalid FX rate. `validate` stops at the first error, so
/// the DeepSeek card below it is dropped too, even though it is fine.
const INVALID_PRICING_SECTION_CONFIG: &str = r#"
[pricing.fx_rates]
CNY = -7.2

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;

#[test]
fn a_rejected_pricing_section_is_labelled_where_the_user_reads_the_price() {
    with_temp_jcode_home(|| {
        write_pricing_config(COMPLETE_CNY_CARD_CONFIG);
        let (_, valid) = widget_cost_line();
        assert!(
            !valid.contains("invalid [pricing]"),
            "a valid section needs no note: {valid}"
        );

        write_pricing_config(INVALID_PRICING_SECTION_CONFIG);
        let (_, labelled) = widget_cost_line();
        assert_eq!(
            labelled.matches("invalid [pricing]").count(),
            1,
            "the rejected section is labelled once per render, not once per note: {labelled}"
        );
        assert!(
            labelled.contains("(invalid [pricing]: pricing.fx_rates.CNY)"),
            "the note names the config path that was rejected: {labelled}"
        );

        // The note describes the section in force, not a fact about the
        // process: fixing the config has to clear it.
        write_pricing_config(COMPLETE_CNY_CARD_CONFIG);
        let (_, recovered) = widget_cost_line();
        assert!(
            !recovered.contains("invalid [pricing]"),
            "the note clears when the section validates again: {recovered}"
        );
    });
}

// Long-context tiers: the tier is selected from the input token count of the
// call's *first* usage snapshot and pinned with the card (F15/F16), so a call
// that grows past a threshold mid-flight is not re-priced.

/// A card whose >200k tier is 10x, so a leaked or missed tier is impossible to
/// miss.
const CONTEXT_TIER_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.0
output = 2.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".context_tiers]]
min_input_tokens = 200_000
multiplier = 10.0
"#;

#[test]
fn remote_call_is_billed_at_the_context_tier_its_first_snapshot_chose() {
    with_temp_jcode_home(|| {
        write_pricing_config(CONTEXT_TIER_CARD_CONFIG);
        let mut app = remote_deepseek_app();

        // First snapshot reports 300k input tokens: above the 200k threshold,
        // so the whole call is billed at the 10x tier.
        app.accrue_remote_call_cost(300_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - 3.0).abs() < 1e-4,
            "300k input at the 10x tier is $3.00, got ${:.4}",
            session_cost_usd(&app)
        );

        // A later delta is still the same call, still the tier its first
        // snapshot chose (the delta alone is below the threshold).
        app.accrue_remote_call_cost(50_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - 3.5).abs() < 1e-4,
            "the pinned tier still prices the delta at 10x, got ${:.4}",
            session_cost_usd(&app)
        );

        // A new call whose first snapshot is below the threshold bills base
        // rates, so the tier is chosen per call rather than leaked.
        app.begin_api_call_accounting();
        let before = session_cost_usd(&app);
        app.accrue_remote_call_cost(100_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - before - 0.1).abs() < 1e-4,
            "100k input on a fresh call is the base $1.00/Mtok, got ${:.4}",
            session_cost_usd(&app) - before
        );
    });
}

#[test]
fn a_call_that_grows_past_the_threshold_keeps_its_first_snapshot_tier() {
    with_temp_jcode_home(|| {
        write_pricing_config(CONTEXT_TIER_CARD_CONFIG);
        let mut app = remote_deepseek_app();

        // First snapshot: 100k, below the threshold -> base tier.
        app.accrue_remote_call_cost(100_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!((session_cost_usd(&app) - 0.1).abs() < 1e-4);

        // The call grows past 200k on a later snapshot, but the card (and with
        // it the tier) was pinned at the first snapshot: no mid-flight upcharge.
        app.accrue_remote_call_cost(300_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (session_cost_usd(&app) - 0.4).abs() < 1e-4,
            "the whole call stays on the base rate it started with, got ${:.4}",
            session_cost_usd(&app)
        );
    });
}

#[test]
fn local_path_bills_the_context_tier_from_the_reported_input_count() {
    with_temp_jcode_home(|| {
        write_pricing_config(CONTEXT_TIER_CARD_CONFIG);

        let local_cost = |input_tokens: u64| {
            let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
            app.streaming.streaming_input_tokens = input_tokens;
            app.streaming.streaming_output_tokens = 0;
            app.begin_call_pricing(instant(INSIDE_PEAK));
            app.update_cost_impl();
            session_cost_usd(&app)
        };

        assert!(
            (local_cost(200_000) - 0.2).abs() < 1e-4,
            "exactly 200k input tokens is still the base tier"
        );
        assert!(
            (local_cost(200_001) - 2.00001).abs() < 1e-3,
            "one token above the threshold bills the 10x tier"
        );
        assert!((local_cost(300_000) - 3.0).abs() < 1e-4);
    });
}

// F8/F20 for `[[pricing.sources]]` sheets: a sheet is the user's own
// configuration too, so a sheet rule that is out of effect must be labelled
// where the user reads the price, exactly like a hand-written card. Without
// this the price silently changes from the user's sheet to models.dev and
// nothing on screen says why (the sheet only logged at `debug`).

/// Write a sheet next to `config.toml` in the isolated home and return the
/// `[[pricing.sources]]` section that points at it.
fn write_source_sheet(id: &str, body: &str) -> String {
    let home = std::env::var_os("JCODE_HOME").expect("test home is set");
    let path = std::path::PathBuf::from(&home).join(format!("{id}.json"));
    std::fs::write(&path, body).expect("write price sheet");
    format!(
        "\n[[pricing.sources]]\nid = \"{id}\"\nfile = \"{}\"\n",
        path.display()
    )
}

#[test]
fn expired_sheet_rule_is_labelled_where_the_user_reads_the_price() {
    with_temp_jcode_home(|| {
        // No config at all: the number the fallback layer produces for this
        // model, which the out-of-effect sheet must fall back to unchanged.
        let (fallback_cost, unlabelled) = widget_cost_line();

        let section = write_source_sheet(
            "expired-sheet",
            r#"{"deepseek":{"models":{"deepseek-v4-pro":{
                "cost":{"input":9.0,"output":18.0},
                "effective_until":"2020-01-01T00:00:00Z"
            }}}}"#,
        );
        write_pricing_config(&section);
        let (sheet_cost, labelled) = widget_cost_line();

        assert!(
            (sheet_cost - fallback_cost).abs() < 1e-4,
            "an out-of-effect sheet prices like the next layer, not at its own 9/18 rate: \
             {sheet_cost} vs {fallback_cost}"
        );
        assert!(
            !unlabelled.contains("pricing source"),
            "sanity: nothing is labelled without a sheet: {unlabelled}"
        );
        assert!(
            labelled.contains("expired"),
            "the widget must say the sheet's rule stopped applying: {labelled}"
        );
        assert!(
            labelled.contains("pricing source `expired-sheet`"),
            "the marker must name the sheet that stopped applying: {labelled}"
        );
    });
}

#[test]
fn an_in_effect_sheet_rule_is_not_labelled() {
    with_temp_jcode_home(|| {
        let section = write_source_sheet(
            "live-sheet",
            r#"{"deepseek":{"models":{"deepseek-v4-pro":{
                "cost":{"input":9.0,"output":18.0},
                "effective_until":"2100-01-01T00:00:00Z"
            }}}}"#,
        );
        write_pricing_config(&section);
        let (cost, line) = widget_cost_line();

        assert!(
            (cost - 27.0).abs() < 1e-3,
            "the in-effect sheet's own 9+18 rates price the call, got ${cost}: {line}"
        );
        assert!(
            !line.contains("expired") && !line.contains("pricing source"),
            "an in-effect sheet must not carry an out-of-effect marker: {line}"
        );
    });
}

// F-A: a `[[pricing.sources]]` sheet's `schedule` is as time-dependent as a
// hand-written card's, but the TUI memoizes the derived price. The memo key has
// to carry the tariff the sheet selects, or a memo filled in one window keeps
// billing that window's rate after the schedule has moved on.

/// A sheet whose peak tariff doubles the base rate, in the same JSON shape a
/// hand-written card states its peak hours in.
const PEAK_SHEET_BODY: &str = r#"{"deepseek":{"models":{"deepseek-v4-pro":{
    "cost":{"input":1.0,"output":2.0},
    "tariffs":{"peak":{"multiplier":2.0}},
    "schedule":[{
        "tariff":"peak",
        "utc_offset_minutes":0,
        "weekdays":["Mon","Tue","Wed","Thu","Fri"],
        "windows":[["01:00","04:00"]]
    }]
}}}}"#;

#[test]
fn a_sheet_schedule_is_read_at_each_calls_instant_not_the_memo_window() {
    with_temp_jcode_home(|| {
        let section = write_source_sheet("peak-sheet", PEAK_SHEET_BODY);
        write_pricing_config(&section);

        let mut app = remote_deepseek_app();

        // First call: Saturday 02:00Z, off-peak, so the sheet's base $1/$2 rates.
        // This also fills the derived-price memo with the off-peak price.
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(FAR_FUTURE_OFF_PEAK));
        assert!(
            (session_cost_usd(&app) - 1.0).abs() < 1e-4,
            "off-peak the sheet's base input rate is $1.00/Mtok, got ${:.4}",
            session_cost_usd(&app)
        );

        // A new call: Monday 02:00Z, inside the sheet's peak window. The memo was
        // filled off-peak; billing must follow the schedule to the peak tariff
        // (2x), not reuse the off-peak price it cached.
        app.begin_api_call_accounting_at(instant(FAR_FUTURE_PEAK));
        let before = session_cost_usd(&app);
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(FAR_FUTURE_PEAK));
        assert!(
            (session_cost_usd(&app) - before - 2.0).abs() < 1e-4,
            "inside the peak window the sheet's input rate doubles to $2.00/Mtok, got ${:.4} \
             (a stale memo would bill the off-peak $1.00)",
            session_cost_usd(&app) - before
        );

        // And back: a call after the window returns to the base rate, so the
        // memo is re-read in both directions rather than pinned to one window.
        app.begin_api_call_accounting_at(instant(FAR_FUTURE_OFF_PEAK));
        let before = session_cost_usd(&app);
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(FAR_FUTURE_OFF_PEAK));
        assert!(
            (session_cost_usd(&app) - before - 1.0).abs() < 1e-4,
            "back off-peak the sheet's base rate applies again, got ${:.4}",
            session_cost_usd(&app) - before
        );
    });
}
