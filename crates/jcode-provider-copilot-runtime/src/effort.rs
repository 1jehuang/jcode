use super::*;

const SONNET5_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const KNOWN_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

impl CopilotApiProvider {
    pub(super) fn efforts_for_model(&self, model: &str) -> Vec<&'static str> {
        let catalog = self.catalog.read();
        match catalog.model_efforts.get(model) {
            Some(advertised) => advertised
                .iter()
                .filter_map(|effort| KNOWN_EFFORTS.iter().copied().find(|known| *known == effort))
                .collect(),
            None if model == "claude-sonnet-5" => SONNET5_EFFORTS.to_vec(),
            None => Vec::new(),
        }
    }

    pub(super) fn reasoning_effort_for_model(&self, model: &str) -> Option<String> {
        let selected = self.reasoning_effort.read();
        let effort = selected.as_ref()?;
        let catalog = self.catalog.read();
        let supported = match catalog.model_efforts.get(model) {
            Some(advertised) => advertised.contains(effort),
            None => model == "claude-sonnet-5" && SONNET5_EFFORTS.contains(&effort.as_str()),
        };
        supported.then(|| effort.clone())
    }

    pub(super) fn add_reasoning_effort_parameter(
        &self,
        body: &mut Value,
        model: &str,
        uses_responses: bool,
    ) {
        if let Some(effort) = self.reasoning_effort_for_model(model) {
            if uses_responses {
                body["reasoning"] = json!({"effort": effort});
            } else {
                body["reasoning_effort"] = json!(effort);
            }
        }
    }
}
