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
            (app.cost.total_cost - 1.0).abs() < 1e-4,
            "off-peak input is $1.00/Mtok, got ${:.4}",
            app.cost.total_cost
        );

        // Delta snapshot inside the peak window: still the same call, so still
        // the off-peak card. Peak output would be $20.00/Mtok.
        app.accrue_remote_call_cost(0, 1_000_000, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (app.cost.total_cost - 3.0).abs() < 1e-4,
            "the whole call bills off-peak ($1.00 + $2.00), got ${:.4}",
            app.cost.total_cost
        );

        // A *new* call at the peak instant really does bill peak rates, so the
        // assertion above pins the tier rather than proving the schedule inert.
        app.begin_api_call_accounting();
        let before = app.cost.total_cost;
        app.accrue_remote_call_cost(1_000_000, 0, 0, 0, instant(INSIDE_PEAK));
        assert!(
            (app.cost.total_cost - before - 10.0).abs() < 1e-4,
            "a call started inside the peak window bills 10x, got ${:.4}",
            app.cost.total_cost - before
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

        assert_eq!(
            app.cost.total_cost, 0.0,
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
            app.cost.total_cost
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
            (app.cost.total_cost - 75.0).abs() < 1e-4,
            "unconfigured unknown models keep the $15/$60 estimate, got ${:.4}",
            app.cost.total_cost
        );
    });
}
