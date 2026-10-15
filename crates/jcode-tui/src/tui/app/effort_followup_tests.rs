use super::{cycled_effort_index, named_profile_reasoning_efforts};

#[test]
fn default_effort_bar_index_excludes_swarm_without_changing_cycling() {
    assert_eq!(
        super::default_effort_bar_index(&["low", "high", "swarm", "swarm-deep"]),
        Some(1)
    );
    assert_eq!(
        super::default_effort_bar_index(&["swarm", "high", "swarm-deep"]),
        Some(1)
    );
    assert_eq!(
        super::default_effort_bar_index(&["swarm", "swarm-deep"]),
        Some(1)
    );
    assert_eq!(super::default_effort_bar_index(&["high"]), Some(0));
    assert_eq!(super::default_effort_bar_index(&[]), None);
}
use crate::config::{Config, NamedProviderConfig, NamedProviderModelConfig};
use jcode_provider_core::{
    DEEPSEEK_SELECTABLE_EFFORTS as DEEPSEEK, OPENAI_SELECTABLE_EFFORTS as OPENAI,
};

fn levels(
    id: &str,
    model: &str,
    profile_effort: Option<bool>,
    model_reasoning: Option<bool>,
    disable_heuristics: bool,
) -> Option<Vec<&'static str>> {
    let mut config = Config::default();
    config.providers.insert(
        id.into(),
        NamedProviderConfig {
            supports_reasoning_effort: profile_effort,
            disable_reasoning_heuristics: disable_heuristics,
            models: vec![NamedProviderModelConfig {
                id: model.into(),
                reasoning: model_reasoning,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    named_profile_reasoning_efforts(
        &config,
        Some(&format!("openai-compatible:{id}")),
        Some(model),
    )
}

macro_rules! profile_case {
    ($name:ident, $id:expr, $model:expr, $profile:expr, $reasoning:expr, $disabled:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                levels($id, $model, $profile, $reasoning, $disabled),
                Some(($expected).to_vec())
            );
        }
    };
}
profile_case!(
    named_deepseek_reasoning_true_keeps_deepseek_ladder,
    "custom",
    "deepseek-v4",
    None,
    Some(true),
    false,
    DEEPSEEK
);
profile_case!(
    named_model_true_overrides_profile_false,
    "custom",
    "kimi-k3",
    Some(false),
    Some(true),
    false,
    OPENAI
);
profile_case!(
    named_model_false_overrides_profile_true,
    "custom",
    "gpt-5.5",
    Some(true),
    Some(false),
    false,
    &[] as &[&str]
);
profile_case!(
    named_profile_true_selects_deepseek_even_for_gpt,
    "custom",
    "gpt-5.5",
    Some(true),
    None,
    false,
    DEEPSEEK
);
profile_case!(
    named_gpt_family_is_inferred,
    "custom",
    "gpt-5.5",
    None,
    None,
    false,
    OPENAI
);
profile_case!(
    named_deepseek_family_is_inferred,
    "custom",
    "deepseek-v4",
    None,
    None,
    false,
    DEEPSEEK
);
profile_case!(
    named_unknown_family_has_no_efforts,
    "custom",
    "kimi-k3",
    None,
    None,
    false,
    &[] as &[&str]
);
profile_case!(
    named_disabled_heuristics_has_no_family_efforts,
    "custom",
    "gpt-5.5",
    None,
    None,
    true,
    &[] as &[&str]
);
profile_case!(
    named_disabled_heuristics_keeps_explicit_model_true,
    "custom",
    "gpt-5.5",
    None,
    Some(true),
    true,
    OPENAI
);
profile_case!(
    named_deepseek_profile_keeps_dedicated_ladder,
    "deepseek",
    "unknown",
    None,
    None,
    true,
    DEEPSEEK
);
profile_case!(
    named_zai_profile_keeps_dedicated_ladder,
    "zai",
    "unknown",
    None,
    None,
    true,
    OPENAI
);
#[test]
fn named_conifer_keeps_existing_inference_delegation() {
    assert_eq!(levels("conifer", "gpt-5.5", None, None, false), None);
}
profile_case!(
    named_conifer_keeps_existing_explicit_disable,
    "conifer",
    "gpt-5.5",
    Some(false),
    Some(false),
    true,
    &[] as &[&str]
);
profile_case!(
    named_conifer_keeps_profile_disable_above_model_enable,
    "conifer",
    "gpt-5.5",
    Some(false),
    Some(true),
    false,
    &[] as &[&str]
);
profile_case!(
    named_conifer_keeps_explicit_profile_ladder,
    "conifer",
    "gpt-5.5",
    Some(true),
    None,
    false,
    DEEPSEEK
);
profile_case!(
    named_conifer_keeps_explicit_model_ladder,
    "conifer",
    "gpt-5.5",
    None,
    Some(true),
    false,
    OPENAI
);
profile_case!(
    named_conifer_keeps_disabled_heuristics,
    "conifer",
    "gpt-5.5",
    None,
    None,
    true,
    &[] as &[&str]
);

#[test]
fn named_model_lookup_trims_and_ignores_case() {
    let mut config = Config::default();
    config.providers.insert(
        "Custom".into(),
        NamedProviderConfig {
            supports_reasoning_effort: Some(false),
            models: vec![NamedProviderModelConfig {
                id: "  KIMI-K3  ".into(),
                reasoning: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    assert_eq!(
        named_profile_reasoning_efforts(
            &config,
            Some("openai-compatible:custom"),
            Some(" kimi-k3 ")
        ),
        Some(OPENAI.to_vec())
    );
}

macro_rules! cycle_case {
    ($name:ident, $ladder:expr, $current:expr, $direction:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let ladder: &[&str] = $ladder;
            assert_eq!(
                cycled_effort_index(ladder, $current, $direction).map(|index| ladder[index]),
                $expected
            );
        }
    };
}
cycle_case!(
    cycle_unknown_up_enters_highest_reasoning,
    OPENAI,
    Some("auto"),
    1,
    Some("max")
);
cycle_case!(
    cycle_unknown_down_enters_below_highest_reasoning,
    OPENAI,
    Some("auto"),
    -1,
    Some("xhigh")
);
cycle_case!(
    cycle_unset_up_enters_highest_reasoning,
    OPENAI,
    None,
    1,
    Some("max")
);
cycle_case!(
    cycle_unset_down_enters_below_highest_reasoning,
    DEEPSEEK,
    None,
    -1,
    Some("high")
);
cycle_case!(
    cycle_minimum_down_wraps_without_entering_swarm,
    OPENAI,
    Some("none"),
    -1,
    Some("max")
);
cycle_case!(
    cycle_explicit_maximum_up_enters_swarm,
    OPENAI,
    Some("max"),
    1,
    Some("swarm")
);
cycle_case!(
    cycle_swarm_up_enters_deep_swarm,
    OPENAI,
    Some("swarm"),
    1,
    Some("swarm-deep")
);
cycle_case!(
    cycle_deep_swarm_up_wraps_to_minimum,
    OPENAI,
    Some("swarm-deep"),
    1,
    Some("none")
);
cycle_case!(
    cycle_swarm_down_returns_to_reasoning,
    OPENAI,
    Some("swarm"),
    -1,
    Some("max")
);
cycle_case!(
    cycle_deep_swarm_down_returns_to_swarm,
    OPENAI,
    Some("swarm-deep"),
    -1,
    Some("swarm")
);
cycle_case!(
    cycle_reasoning_up_advances_one_rung,
    OPENAI,
    Some("low"),
    1,
    Some("medium")
);
cycle_case!(
    cycle_reasoning_down_advances_one_rung,
    OPENAI,
    Some("high"),
    -1,
    Some("medium")
);
cycle_case!(
    cycle_without_swarm_wraps_up,
    &["low", "high"],
    Some("high"),
    1,
    Some("low")
);
cycle_case!(
    cycle_without_swarm_wraps_down,
    &["low", "high"],
    Some("low"),
    -1,
    Some("high")
);
cycle_case!(
    cycle_singleton_keeps_only_rung,
    &["high"],
    Some("high"),
    1,
    Some("high")
);
cycle_case!(
    cycle_unknown_singleton_down_enters_only_rung,
    &["high"],
    None,
    -1,
    Some("high")
);
cycle_case!(cycle_empty_ladder_returns_none, &[], None, 1, None);
cycle_case!(
    cycle_swarm_only_unknown_uses_last_sentinel,
    &["swarm", "swarm-deep"],
    None,
    1,
    Some("swarm-deep")
);
cycle_case!(
    cycle_swarm_only_last_does_not_wrap,
    &["swarm", "swarm-deep"],
    Some("swarm-deep"),
    1,
    Some("swarm-deep")
);
