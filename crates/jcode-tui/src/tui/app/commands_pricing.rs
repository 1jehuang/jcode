//! `/pricing` - what prices the current model, and where that number comes from.
//!
//! A call is priced by whichever layer wins: the hand-written `[pricing]` card,
//! the curated tables, models.dev, or the fallback estimate. That choice was
//! invisible in the UI, which is why "why is it this price" had no answer short
//! of reading the log. This command prints the winning layer for the current
//! model, the tariff in force, the currency, and the state of the rate table.

use super::*;

/// What the `[pricing]` layer says about the current model.
pub(super) enum CardState<'a> {
    /// A hand-written card prices this model, with `tariff` already applied.
    Priced {
        card: &'a crate::model_pricing::CallRateCard,
        tariff: Option<&'a str>,
    },
    /// A card claims the pair but cannot price the call: incomplete in a
    /// currency that cannot merge with the next layer, or expired with
    /// `on_rule_expiry = "no_price"`. The display has to say so rather than
    /// echo a number from a layer the user did not ask for.
    ConfiguredWithoutPrice,
    /// No hand-written rule claims this model.
    Absent,
}

/// Everything `/pricing` reports, gathered before formatting so the text can be
/// asserted without an `App`.
pub(super) struct PricingReport<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub card: CardState<'a>,
    /// Why the `[pricing]` section was rejected, when it was.
    pub config_error: Option<&'a str>,
    /// The user's own rule that covers this model but is out of its validity
    /// window at the report instant: a `[pricing.providers]` card or a
    /// `[[pricing.sources]]` sheet (whose label names the sheet).
    pub out_of_effect: Option<&'a crate::model_pricing::PricingNotice>,
    pub fx_base: &'a jcode_provider_core::Currency,
    pub fx_rates: &'a std::collections::BTreeMap<jcode_provider_core::Currency, f64>,
    pub display_currency: &'a str,
    /// Canonical-request cost from whichever layer wins, when one can be had.
    pub reference_cost: Option<&'a jcode_provider_core::Money>,
    /// Long-context thresholds the winning card declares, in declaration order.
    ///
    /// This report has no token count and therefore prices the **base tier**.
    /// Listing the thresholds is what keeps that honest: the user learns which
    /// tiers a real call can cross instead of reading the figure as the only one.
    pub context_tier_thresholds: &'a [u64],
    /// The `[[pricing.sources]]` sheet that prices the model at the report
    /// instant, as `(sheet_id, currency)`, when no `[pricing.providers]` card
    /// does. The sheet layer sits between the card and models.dev, so a report
    /// whose `reference request` figure came from a sheet must name it here
    /// rather than claim only models.dev/provider caches could be the source.
    pub sheet: Option<(&'a str, jcode_provider_core::Currency)>,
}

impl PricingReport<'_> {
    pub(super) fn render(&self) -> String {
        let mut out = format!("**Pricing · {} / {}**\n", self.provider, self.model);

        match &self.card {
            CardState::Priced { card, tariff } => {
                let mut parts = vec![
                    format!(
                        "{} in · {} out",
                        crate::money_display::format_amount(card.input_per_mtok, &card.currency, 2),
                        crate::money_display::format_amount(card.output_per_mtok, &card.currency, 2),
                    ),
                    match card.cache_read_per_mtok {
                        Some(rate) => format!(
                            "{} cache-read",
                            crate::money_display::format_amount(rate, &card.currency, 2)
                        ),
                        None => "no cache-read rate".to_string(),
                    },
                ];
                if card.cache_write_per_mtok.is_some() {
                    parts.push("cache-write set".to_string());
                }
                let tariff = match tariff {
                    Some(name) => format!("`{name}` tariff"),
                    None => "card's own rates".to_string(),
                };
                out.push_str(&format!(
                    "\n- rate card: {} per Mtok ({tariff}, {})\n- from: your `[pricing.providers]` card, which outranks every other source\n",
                    parts.join(" · "),
                    card.currency.as_str(),
                ));
            }
            CardState::ConfiguredWithoutPrice => out.push_str(
                "\n- rate card: **unknown** - your `[pricing]` card claims this model but cannot price it \
                 (an incomplete card in its own currency, or a rule that expired with `on_rule_expiry = \"no_price\"`). \
                 No other layer is substituted.\n",
            ),
            CardState::Absent => {
                out.push_str(
                    "\n- rate card: no `[pricing]` rule prices this model, so the cost comes from a lower layer \
                     (a `[[pricing.sources]]` sheet, models.dev, a provider cache, or the fallback estimate)\n",
                );
                if let Some((sheet_id, currency)) = self.sheet.as_ref() {
                    out.push_str(&format!(
                        "- from: [[pricing.sources]] sheet `{sheet_id}` ({})\n",
                        currency.as_str()
                    ));
                }
            }
        }

        if let Some(notice) = self.out_of_effect {
            out.push_str(&format!(
                "- out of effect: {} - the cost above comes from a lower layer, not from your rule\n",
                notice.label()
            ));
        }

        if let Some(cost) = self.reference_cost {
            out.push_str(&format!(
                "- reference request (25k in / 5k out): {}\n",
                crate::money_display::format_amount(cost.amount, &cost.currency, 4)
            ));
        } else {
            out.push_str(
                "- reference request: **unpriced** - no layer can price this model right now\n",
            );
        }

        if !self.context_tier_thresholds.is_empty() {
            let tiers = self
                .context_tier_thresholds
                .iter()
                .map(|threshold| format!(">{threshold} input tokens"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "- long-context tiers: {tiers} (the rates above are the base tier; a call whose \
                 first usage snapshot reports more than a threshold is billed at that tier)\n"
            ));
        }

        if self.fx_rates.is_empty() {
            out.push_str(&format!(
                "- fx: no rates configured, so every cost stays in its own currency ({}) and the model \
                 picker skips cross-currency ordering\n",
                self.fx_base.as_str()
            ));
        } else {
            let rates = self
                .fx_rates
                .iter()
                .map(|(currency, rate)| format!("{} {rate}", currency.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "- fx: 1 {} = {rates} (used for display conversion and for ordering the model picker)\n",
                self.fx_base.as_str()
            ));
        }

        out.push_str(&format!(
            "- display currency: `{}`\n",
            self.display_currency
        ));

        match self.config_error {
            Some(error) => out.push_str(&format!(
                "- config: the `[pricing]` section was **rejected** ({error}), so every rule in it is ignored\n"
            )),
            None => out.push_str("- config: `[pricing]` accepted\n"),
        }

        out
    }
}

pub(super) fn handle_pricing_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/pricing") else {
        return false;
    };
    if !rest.trim().is_empty() {
        return false;
    }

    let now = std::time::SystemTime::now();
    let (model, is_anthropic, is_openai) = app.remote_billing_identity();
    let source_key = app.billing_source_key(is_anthropic, is_openai);
    let provider = <App as crate::tui::TuiState>::provider_name(app).to_string();
    let tariff = crate::model_pricing::selected_config_tariff(&source_key, &model, now);
    let context_tier_thresholds =
        crate::model_pricing::context_tier_thresholds(&source_key, &model, now);

    // One lookup, then own the card so the report can borrow all of it. `None`
    // is deliberate: this report is a reference request with no size, so it
    // prices the base tier and names the tiers separately below.
    let rates_state = crate::model_pricing::config_call_rates(&source_key, &model, now, None);
    let priced = match &rates_state {
        crate::model_pricing::ConfigCallRates::Priced(card) => Some(card.clone()),
        _ => None,
    };
    // F8/F20: which of the user's own rules stopped applying to this model. The
    // card first (it outranks a sheet), then the sheet layer, so `/pricing`
    // answers "why is it this price" the same way the cost widget does.
    let out_of_effect = match &rates_state {
        crate::model_pricing::ConfigCallRates::OutOfEffect(reason) => {
            Some(crate::model_pricing::PricingNotice::ConfigCard(*reason))
        }
        crate::model_pricing::ConfigCallRates::Absent => {
            crate::model_pricing::sheet_rule_out_of_effect(&source_key, &model, now)
        }
        _ => None,
    };
    let card = match priced.as_ref() {
        Some(card) => CardState::Priced {
            card,
            tariff: tariff.as_deref(),
        },
        None => match rates_state {
            crate::model_pricing::ConfigCallRates::ConfiguredWithoutPrice => {
                CardState::ConfiguredWithoutPrice
            }
            _ => CardState::Absent,
        },
    };

    let pricing = crate::model_pricing::pricing_config();
    let error = crate::model_pricing::pricing_config_error();
    let reference = crate::model_pricing::effective_cost(&source_key, &model, now);
    let display_currency = crate::config::config().display.currency.clone();
    // The sheet layer sits between the card and models.dev; name it when it is
    // the layer that priced the model, so the report cannot deny a source its
    // own `reference request` figure came from.
    let sheet = crate::model_pricing::source_sheet_for(&source_key, &model, now);
    let sheet_label = sheet
        .as_ref()
        .map(|(id, currency)| (id.as_str(), currency.clone()));

    let report = PricingReport {
        provider: &provider,
        model: &model,
        card,
        config_error: error.as_ref().map(|error| error.message.as_str()),
        out_of_effect: out_of_effect.as_ref(),
        fx_base: &pricing.fx_base,
        fx_rates: &pricing.fx_rates,
        display_currency: &display_currency,
        reference_cost: reference.as_ref(),
        context_tier_thresholds: &context_tier_thresholds,
        sheet: sheet_label,
    };
    app.push_display_message(DisplayMessage::system(report.render()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_provider_core::{Currency, Money};

    fn card() -> crate::model_pricing::CallRateCard {
        crate::model_pricing::CallRateCard {
            input_per_mtok: 2.0,
            output_per_mtok: 8.0,
            cache_read_per_mtok: Some(0.04),
            cache_write_per_mtok: None,
            currency: Currency::new("CNY"),
        }
    }

    /// The priced case names the layer, the tariff and the currency, and never
    /// prints a CNY amount as dollars.
    #[test]
    fn a_config_priced_model_reports_its_tariff_and_currency() {
        let card = card();
        let rates = std::collections::BTreeMap::from([(Currency::new("CNY"), 7.2)]);
        let report = PricingReport {
            provider: "DeepSeek",
            model: "deepseek-flash",
            card: CardState::Priced {
                card: &card,
                tariff: Some("peak"),
            },
            config_error: None,
            out_of_effect: None,
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "native",
            reference_cost: Some(&Money::new(0.09, Currency::new("CNY"))),
            context_tier_thresholds: &[],
            sheet: None,
        };
        let text = report.render();

        assert!(text.contains("CNY 2.00 in"), "{text}");
        assert!(text.contains("CNY 8.00 out"), "{text}");
        assert!(text.contains("peak"), "{text}");
        assert!(
            !text.contains('$'),
            "a CNY card must not be labelled with a dollar sign: {text}"
        );
        assert!(text.contains("CNY 0.0900"), "{text}");
        assert!(text.contains("1 USD = CNY 7.2"), "{text}");
        assert!(text.contains("`[pricing]` accepted"), "{text}");
    }

    /// A card that claims the model but cannot price it must say "unknown"
    /// rather than borrow a number from another layer.
    #[test]
    fn a_card_that_cannot_price_says_unknown() {
        let rates = std::collections::BTreeMap::new();
        let report = PricingReport {
            provider: "DeepSeek",
            model: "deepseek-v4-pro",
            card: CardState::ConfiguredWithoutPrice,
            config_error: None,
            out_of_effect: None,
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "native",
            reference_cost: None,
            context_tier_thresholds: &[],
            sheet: None,
        };
        let text = report.render();

        assert!(text.contains("**unknown**"), "{text}");
        assert!(text.contains("unpriced"), "{text}");
        assert!(
            text.contains("no rates configured"),
            "an empty fx table must be stated, not implied: {text}"
        );
    }

    /// A rejected `[pricing]` section is why every hand-written rule stopped
    /// applying, so the report has to lead with it.
    #[test]
    fn a_rejected_section_is_reported() {
        let rates = std::collections::BTreeMap::new();
        let report = PricingReport {
            provider: "OpenAI",
            model: "gpt-5.5",
            card: CardState::Absent,
            config_error: Some("pricing.schedule[0].windows: window end must be after start"),
            out_of_effect: None,
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "CNY",
            reference_cost: None,
            context_tier_thresholds: &[],
            sheet: None,
        };
        let text = report.render();

        assert!(
            text.contains("no `[pricing]` rule prices this model"),
            "{text}"
        );
        assert!(text.contains("**rejected**"), "{text}");
        assert!(text.contains("window end must be after start"), "{text}");
        assert!(text.contains("display currency: `CNY`"), "{text}");
    }

    /// `/pricing` has no token count, so it prices the base tier. It has to name
    /// the declared thresholds, otherwise the base figure reads as the only one.
    #[test]
    fn a_long_context_card_names_its_tiers_and_the_base_tier_caveat() {
        let card = card();
        let rates = std::collections::BTreeMap::new();
        let thresholds = [200_000u64, 500_000];
        let report = PricingReport {
            provider: "DeepSeek",
            model: "deepseek-v4-pro",
            card: CardState::Priced {
                card: &card,
                tariff: None,
            },
            config_error: None,
            out_of_effect: None,
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "native",
            reference_cost: None,
            context_tier_thresholds: &thresholds,
            sheet: None,
        };
        let text = report.render();

        assert!(text.contains(">200000 input tokens"), "{text}");
        assert!(text.contains(">500000 input tokens"), "{text}");
        assert!(text.contains("base tier"), "{text}");
    }

    /// F8/F20 for sheets: `/pricing` answers "why is it this price" the same way
    /// the cost widget does, so an out-of-effect sheet rule must be named here
    /// too - and the label must name *which* sheet stopped applying.
    #[test]
    fn an_out_of_effect_sheet_rule_is_reported_by_name() {
        let notice = crate::model_pricing::PricingNotice::PriceSheet {
            source_id: "deepseek-mirror".to_string(),
            reason: crate::model_pricing::RuleOutOfEffect::Expired,
        };
        let rates = std::collections::BTreeMap::new();
        let report = PricingReport {
            provider: "DeepSeek",
            model: "deepseek-v4-pro",
            card: CardState::Absent,
            config_error: None,
            out_of_effect: Some(&notice),
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "native",
            reference_cost: None,
            context_tier_thresholds: &[],
            sheet: None,
        };
        let text = report.render();

        assert!(text.contains("out of effect"), "{text}");
        assert!(text.contains("rule expired"), "{text}");
        assert!(
            text.contains("pricing source `deepseek-mirror`"),
            "the report must name the sheet that stopped applying: {text}"
        );
        assert!(
            text.contains("comes from a lower layer"),
            "and say the price is another layer's: {text}"
        );
    }

    /// The card's own marker still reads exactly as before: the sheet label is
    /// an addition, not a change to the card's wording.
    #[test]
    fn a_config_card_marker_keeps_its_wording() {
        let notice = crate::model_pricing::PricingNotice::ConfigCard(
            crate::model_pricing::RuleOutOfEffect::NotYetEffective,
        );
        assert_eq!(notice.label(), "rule not in effect yet");
    }

    /// When no card prices the model but a `[[pricing.sources]]` sheet does, the
    /// report must name that sheet: otherwise it lists only "models.dev, a
    /// provider cache, or the fallback estimate" while the `reference request`
    /// figure right beside it came from the sheet.
    #[test]
    fn an_absent_card_names_the_sheet_that_priced_the_model() {
        let rates = std::collections::BTreeMap::new();
        let report = PricingReport {
            provider: "DeepSeek",
            model: "deepseek-v4-pro",
            card: CardState::Absent,
            config_error: None,
            out_of_effect: None,
            fx_base: &Currency::usd(),
            fx_rates: &rates,
            display_currency: "native",
            reference_cost: Some(&Money::new(0.005, Currency::new("CNY"))),
            context_tier_thresholds: &[],
            sheet: Some(("corp-mirror", Currency::new("CNY"))),
        };
        let text = report.render();

        assert!(
            text.contains("[[pricing.sources]] sheet `corp-mirror` (CNY)"),
            "the sheet layer must be named: {text}"
        );
        assert!(
            text.contains("a `[[pricing.sources]]` sheet,"),
            "and it must appear in the list of lower layers: {text}"
        );
    }
}
