//! ACP form-mode payloads. Validation is independent of any client transport.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormElicitation {
    pub elicitation_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub message: String,
    pub requested_schema: FormSchema,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ElicitationResponse {
    Accept {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Map<String, Value>>,
    },
    Decline,
    Cancel,
}

/// Flat, bounded ACP schema. Call `validate` before publishing a form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormSchema(pub Value);

impl FormSchema {
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(&self.0)
            .map_err(|e| e.to_string())?
            .len()
            > 65536
        {
            return Err("form schema exceeds 64 KiB".into());
        }
        static SCHEMA: std::sync::OnceLock<Result<jsonschema::Validator, String>> =
            std::sync::OnceLock::new();
        let validator = SCHEMA
            .get_or_init(|| {
                let schema: Value = serde_json::from_str(include_str!("elicitation-schema.json"))
                    .map_err(|error| error.to_string())?;
                jsonschema::validator_for(&schema).map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        validator
            .validate(&self.0)
            .map_err(|error| error.to_string())?;
        if self.0.get("type").and_then(Value::as_str) != Some("object") {
            return Err("form schema must be an object".into());
        }
        check_limits(&self.0)?;
        let properties = self
            .0
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("form properties must be an object")?;
        if properties.len() > 64 {
            return Err("form exceeds 64 fields".into());
        }
        for (name, property) in properties {
            if name.len() > 256 {
                return Err("form field name exceeds 256 bytes".into());
            }
            match property.get("type").and_then(Value::as_str) {
                Some("string" | "number" | "integer" | "boolean") => {}
                Some("array") => {
                    let items = property.get("items").ok_or("array requires items")?;
                    if items.get("type").and_then(Value::as_str) != Some("string")
                        && !(items.get("type").is_none() && items.get("anyOf").is_some())
                    {
                        return Err(format!("{name}: only string selections are supported"));
                    }
                }
                _ => return Err(format!("{name}: unsupported form field type")),
            }
            for key in ["properties", "additionalProperties", "$ref", "$dynamicRef"] {
                if property.get(key).is_some() {
                    return Err(format!("{name}: unsupported schema keyword {key}"));
                }
            }
            check_limits(property)?;
        }
        if let Some(required) = self.0.get("required").filter(|v| !v.is_null()) {
            for name in required.as_array().ok_or("required must be an array")? {
                if !name
                    .as_str()
                    .is_some_and(|name| properties.contains_key(name))
                {
                    return Err("required names an unknown field".into());
                }
            }
        }
        jsonschema::validator_for(&self.content_schema()).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn check_content(&self, content: &Map<String, Value>) -> Result<(), String> {
        self.validate()?;
        let validator =
            jsonschema::validator_for(&self.content_schema()).map_err(|e| e.to_string())?;
        validator
            .validate(&Value::Object(content.clone()))
            .map_err(|e| e.to_string())
    }

    fn content_schema(&self) -> Value {
        let mut schema = self.0.clone();
        normalize_nullable_keywords(&mut schema);
        schema["additionalProperties"] = Value::Bool(false);
        schema
    }
}

// ACP treats null optional constraints as absent; JSON Schema does not.
fn normalize_nullable_keywords(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, value| {
                !value.is_null()
                    || !matches!(
                        key.as_str(),
                        "required"
                            | "minLength"
                            | "maxLength"
                            | "minimum"
                            | "maximum"
                            | "minItems"
                            | "maxItems"
                            | "pattern"
                            | "format"
                            | "enum"
                            | "oneOf"
                    )
            });
            for value in object.values_mut() {
                normalize_nullable_keywords(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_nullable_keywords(value);
            }
        }
        _ => {}
    }
}

fn check_limits(value: &Value) -> Result<(), String> {
    match value {
        Value::String(text) if text.len() > 8192 => Err("form string exceeds 8192 bytes".into()),
        Value::Array(items) => {
            if items.len() > 256 {
                return Err("form exceeds 256 options".into());
            }
            for item in items {
                check_limits(item)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "$ref" | "$dynamicRef") {
                    return Err("schema references are not supported".into());
                }
                check_limits(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_flat_fields_and_rejects_invalid_content() {
        let schema = FormSchema(json!({"type":"object", "properties": {
            "name":{"type":"string", "minLength":2, "pattern":"^[a-z]+$"},
            "count":{"type":"integer", "minimum":1, "maximum":3},
            "ok":{"type":"boolean"},
            "choice":{"type":"string", "oneOf":[{"const":"a","title":"A"}]},
            "tags":{"type":"array", "items":{"type":"string","enum":["x","y"]}, "maxItems":2}
        }, "required":["name"]}));
        assert!(schema.validate().is_ok());
        let valid = json!({"name":"ab","count":2,"ok":true,"choice":"a","tags":["x"]});
        assert!(schema.check_content(valid.as_object().unwrap()).is_ok());
        for invalid in [
            json!({}),
            json!({"name":"A"}),
            json!({"name":"ab","extra":1}),
            json!({"name":"ab","count":1.5}),
            json!({"name":"ab","count":4}),
            json!({"name":"ab","choice":"b"}),
            json!({"name":"ab","tags":["z"]}),
        ] {
            assert!(
                schema.check_content(invalid.as_object().unwrap()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn rejects_nested_unknown_oversize_and_bad_required_schemas() {
        for property in [
            json!({"type":"object"}),
            json!({"type":"future"}),
            json!({"type":"array","items":{"type":"object"}}),
            json!({"type":"string","pattern":"["}),
        ] {
            assert!(
                FormSchema(json!({"type":"object","properties":{"x":property}}))
                    .validate()
                    .is_err()
            );
        }
        assert!(
            FormSchema(json!({"type":"object","properties":{},"required":["missing"]}))
                .validate()
                .is_err()
        );
        assert!(FormSchema(json!({"type":"object","properties":{"x":{"type":"string","title":"x".repeat(8193)}}})).validate().is_err());
    }

    #[test]
    fn preserves_pinned_titled_multiselect_and_nullable_constraints() {
        let schema = FormSchema(json!({"type":"object", "required":null, "properties": {
            "tags":{"type":"array", "minItems":null, "items":{"anyOf":[
                {"const":"x","title":"X"}, {"const":"y","title":"Y"}]}},
            "text":{"type":"string", "minLength":null,"enum":null,"pattern":null}
        }}));
        assert!(schema.validate().is_ok());
        assert!(
            schema
                .check_content(json!({"tags":["x"],"text":"ok"}).as_object().unwrap())
                .is_ok()
        );
        assert!(
            schema
                .check_content(json!({"tags":["z"]}).as_object().unwrap())
                .is_err()
        );
        let request = FormElicitation {
            elicitation_id: "opaque".into(),
            session_id: "session".into(),
            tool_call_id: Some("call".into()),
            message: "Choose".into(),
            requested_schema: schema,
        };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<FormElicitation>(encoded).unwrap(),
            request
        );
    }

    #[test]
    fn response_roundtrips_and_distinguishes_decline_cancel() {
        for value in [
            json!({"action":"accept"}),
            json!({"action":"accept","content":{"x":true}}),
            json!({"action":"decline"}),
            json!({"action":"cancel"}),
        ] {
            let response: ElicitationResponse = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), value);
        }
        assert!(
            serde_json::from_value::<ElicitationResponse>(json!({"action":"accept","content":[]}))
                .is_err()
        );
    }
}

/// Terminal broker outcome, broadcast to withdraw forms on peer clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationResolution {
    Accepted,
    Declined,
    Cancelled,
    Expired,
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::{Request, ServerEvent};
    use serde_json::json;
    #[test]
    fn elicitation_wire_roundtrips_and_legacy_subscribe_omits_false() {
        let request = Request::ElicitationResponse {
            id: 7,
            session_id: "s".into(),
            elicitation_id: "e".into(),
            response: ElicitationResponse::Decline,
        };
        assert_eq!(request.id(), 7);
        assert!(request.is_lightweight_control_request());
        let encoded = serde_json::to_value(&request).unwrap();
        let decoded: Request = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
        let event = ServerEvent::ElicitationResolved {
            session_id: "s".into(),
            elicitation_id: "e".into(),
            outcome: ElicitationResolution::Accepted,
        };
        let encoded = serde_json::to_value(&event).unwrap();
        let decoded: ServerEvent = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
        let legacy: Request = serde_json::from_value(json!({"type":"subscribe","id":1})).unwrap();
        let encoded = serde_json::to_value(legacy).unwrap();
        assert!(encoded.get("supports_elicitation_form").is_none());
        assert!(encoded.get("applet_elicitation").is_none());
    }
}
