use super::{parse_bracket_setting, remove_bracket_setting, string_value};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub(super) struct Selector {
    pub id: String,
    pub current: Option<String>,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModelCatalog {
    pub models: Vec<String>,
    pub current: Option<String>,
    pub config_id: Option<String>,
    pub effort: Option<Selector>,
}

impl ModelCatalog {
    pub fn unconfirmed(&self) -> Self {
        let mut catalog = self.clone();
        catalog.current = None;
        if let Some(effort) = &mut catalog.effort {
            effort.current = None;
        }
        catalog
    }

    pub fn merge(&mut self, value: &Value) {
        if let Some(models) = value.get("models") {
            let choices = model_ids(models.get("availableModels").unwrap_or(models));
            if !choices.is_empty() {
                self.models = choices;
                self.current = string_value(models.get("currentModelId"));
            }
        }
        if let Some(options) = value.get("configOptions").and_then(Value::as_array) {
            let model = options.iter().find(|o| is_selector(o, &["model"]));
            if let Some(option) = model {
                let choices = model_ids(&option["options"]);
                if !choices.is_empty()
                    && string_value(option.get("id")).is_some()
                    && string_value(option.get("currentValue"))
                        .is_some_and(|v| choices.contains(&v))
                {
                    self.models = choices;
                    self.config_id = string_value(option.get("id"));
                    self.current = string_value(option.get("currentValue"));
                    self.effort = options
                        .iter()
                        .find(|o| {
                            is_selector(
                                o,
                                &["thought_level", "effort", "reasoning", "reasoning_effort"],
                            )
                        })
                        .and_then(selector);
                }
            }
        }
        if let Some(id) = value.get("configId").and_then(Value::as_str) {
            let current = string_value(value.get("value"))
                .or_else(|| string_value(value.get("currentValue")));
            if self.config_id.as_deref() == Some(id) || id == "model" {
                let choices = model_ids(&value["options"]);
                if !choices.is_empty() {
                    self.models = choices;
                }
                if current.as_ref().is_some_and(|v| self.models.contains(v)) {
                    self.current = current;
                }
                self.config_id = Some(id.into());
            } else if let Some(effort) = self.effort.as_mut().filter(|s| s.id == id) {
                let choices = model_ids(&value["options"]);
                if !choices.is_empty() {
                    effort.choices = choices;
                }
                if current.as_ref().is_some_and(|v| effort.choices.contains(v)) {
                    effort.current = current;
                }
            }
        }
    }

    pub fn effort_choices(&self, model: &str) -> Vec<String> {
        if let Some(selector) = &self.effort {
            return selector.choices.clone();
        }
        self.models
            .iter()
            .filter(|id| effort_base(id) == effort_base(model))
            .map(|id| bracket_effort(id).unwrap_or_else(|| "none".into()))
            .collect()
    }
}

pub(super) fn effort_base(model: &str) -> String {
    remove_bracket_setting(&remove_bracket_setting(model, "effort"), "reasoning")
}

pub(super) fn bracket_effort(model: &str) -> Option<String> {
    parse_bracket_setting(model, "effort").or_else(|| parse_bracket_setting(model, "reasoning"))
}

fn is_selector(value: &Value, names: &[&str]) -> bool {
    ["category", "id"].iter().any(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| names.contains(&s))
    })
}

fn selector(value: &Value) -> Option<Selector> {
    let choices = model_ids(&value["options"]);
    if choices.is_empty() {
        return None;
    }
    let current = string_value(value.get("currentValue"))?;
    if !choices.contains(&current) {
        return None;
    }
    Some(Selector {
        id: string_value(value.get("id"))?,
        current: Some(current),
        choices,
    })
}

pub(super) fn model_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for item in value.as_array().into_iter().flatten() {
        if item.get("options").is_some() {
            for id in model_ids(&item["options"]) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        } else if let Some(id) = item
            .as_str()
            .map(str::to_string)
            .or_else(|| string_value(item.get("modelId")))
            .or_else(|| string_value(item.get("value")))
            .or_else(|| string_value(item.get("id")))
        {
            let id = id.trim().to_string();
            if !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}
