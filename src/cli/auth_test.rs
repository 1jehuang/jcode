use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const DEFAULT_AUTH_TEST_PROVIDER_PROMPT: &str =
    "Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools.";
const AUTH_TEST_TOOL_NAME: &str = "bash";
const AUTH_TEST_TOOL_COMMAND: &str = "echo JCODE_TOOL_OK";
const AUTH_TEST_TOOL_OUTPUT_MARKER: &str = "JCODE_TOOL_OK";
const DEFAULT_AUTH_TEST_TOOL_PROMPT: &str = "Use exactly one bash tool call with command exactly `echo JCODE_TOOL_OK`. After you see the tool result, reply with exactly AUTH_TEST_OK and nothing else.";

include!("auth_test/types.rs");
include!("auth_test/run.rs");
include!("auth_test/probes.rs");
include!("auth_test/choice.rs");
