use jcode_tool_types::{ToolImage, ToolOutput};
use serde::Serialize;

/// Channel for sending provider-native tool results back to a provider bridge.
pub type NativeToolResultSender = tokio::sync::mpsc::Sender<NativeToolResult>;

/// Native tool result to send back to provider bridges that delegate tool execution to jcode.
#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResult {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub request_id: String,
    pub result: NativeToolResultPayload,
    pub is_error: bool,
    /// In-process attachments; provider bridges encode their own image blocks.
    #[serde(skip)]
    pub images: Vec<ToolImage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResultPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NativeToolResult {
    pub fn success(request_id: String, output: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: Some(output),
                error: None,
            },
            is_error: false,
            images: Vec::new(),
        }
    }

    pub fn from_tool_output(request_id: String, output: ToolOutput) -> Self {
        let mut result = Self::success(request_id, output.output);
        result.images = output.images;
        result
    }

    pub fn error(request_id: String, error: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: None,
                error: Some(error),
            },
            is_error: true,
            images: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_preserve_the_generic_native_wire_shape() {
        let output = ToolOutput::new("captured").with_image("image/png", "cG5n");
        let result = NativeToolResult::from_tool_output("request".into(), output);
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"type":"native_tool_result", "request_id":"request",
                "result":{"output":"captured"}, "is_error":false})
        );
    }
}
