//! DAP (Debug Adapter Protocol) 协议定义模块
//! 
//! 实现 Debug Adapter Protocol 的类型定义，用于与 IDE/编辑器进行调试通信

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type CommandId = i64;
pub type ThreadId = i64;
pub type StackFrameId = i64;
pub type ScopeId = i64;
pub type VariableId = i64;
pub type BreakpointId = String;
pub type Source = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Option<CommandId>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<ResponseError>,
    pub jsonrpc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub adapter_id: Option<String>,
    pub locale: Option<String>,
    pub lines_start_at1: Option<bool>,
    pub columns_start_at1: Option<bool>,
    pub path_format: Option<String>,
    pub supports_variable_type: Option<bool>,
    pub supports_variable_paging: Option<bool>,
    pub supports_run_in_terminal_request: Option<bool>,
    pub supports_memory_references: Option<bool>,
    pub supports_progress_reporting: Option<bool>,
    pub supports_invalidated_event: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub supports_configuration_done_request: Option<bool>,
    pub supports_launch_request: Option<bool>,
    pub supports_attach_request: Option<bool>,
    pub supports_restart_request: Option<bool>,
    pub supports_set_breakpoints_request: Option<bool>,
    pub supports_set_exception_breakpoints_request: Option<bool>,
    pub supports_delayed_stack_trace_loading: Option<bool>,
    pub supports_data_breakpoints: Option<bool>,
    pub supports_conditional_breakpoints: Option<bool>,
    pub supports_log_breakpoints: Option<bool>,
    pub supports_evaluate_for_hovers: Option<bool>,
    pub exception_breakpoint_filters: Option<Vec<ExceptionBreakpointFilter>>,
    pub supports_step_back: Option<bool>,
    pub supports_set_expression: Option<bool>,
    pub supports_set_variable: Option<bool>,
    pub supports_terminate_request: Option<bool>,
    pub supports_terminate_thread_request: Option<bool>,
    pub supports_pause_on_exit_request: Option<bool>,
    pub supports_read_memory_request: Option<bool>,
    pub supports_disassemble_request: Option<bool>,
    pub supports_cancel_request: Option<bool>,
    pub supports_output_event: Option<bool>,
    pub supports_progress_start_event: Option<bool>,
    pub supports_progress_update_event: Option<bool>,
    pub supports_progress_end_event: Option<bool>,
    pub supports_invalidated_event: Option<bool>,
    pub supports_memory_event: Option<bool>,
    pub supports_reverse_continue: Option<bool>,
    pub supports_single_thread_execution_requests: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionBreakpointFilter {
    pub filter: String,
    pub label: String,
    pub default: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub program: Option<String>,
    pub args: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub env_file: Option<String>,
    pub stop_on_entry: Option<bool>,
    pub console: Option<String>,
    pub debug_port: Option<u16>,
    pub request: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachRequest {
    pub request: String,
    pub pid: Option<i32>,
    pub port: Option<u16>,
    pub host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsRequest {
    pub source: SourceDescriptor,
    pub breakpoints: Option<Vec<SourceBreakpoint>>,
    pub source_modified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescriptor {
    pub name: Option<String>,
    pub path: Option<String>,
    pub source_reference: Option<i64>,
    pub origin: Option<String>,
    pub presentation_hint: Option<PresentationHint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationHint {
    pub kind: Option<String>,
    pub title: Option<String>,
    pub root: Option<String>,
    pub presentation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    pub column: Option<i64>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsResponse {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    pub id: Option<BreakpointId>,
    pub verified: bool,
    pub line: Option<i64>,
    pub column: Option<i64>,
    pub source: Option<SourceDescriptor>,
    pub message: Option<String>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
    pub disabled: Option<bool>,
    pub pending: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetExceptionBreakpointsRequest {
    pub filters: Vec<String>,
    pub exception_options: Option<Vec<ExceptionOptions>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionOptions {
    pub path: SourceDescriptor,
    pub break_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsResponse {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: ThreadId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceRequest {
    pub thread_id: ThreadId,
    pub start_frame: Option<i64>,
    pub levels: Option<i64>,
    pub format: Option<StackTraceFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceFormat {
    pub parameters: Option<bool>,
    pub parameter_types: Option<bool>,
    pub parameter_names: Option<bool>,
    pub parameter_values: Option<bool>,
    pub line: Option<bool>,
    pub module: Option<bool>,
    pub include_all_scopes: Option<bool>,
    pub variables: Option<bool>,
    pub source: Option<bool>,
    pub source_origin: Option<bool>,
    pub thread_id: Option<bool>,
    pub column: Option<bool>,
    pub end_line: Option<bool>,
    pub end_column: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceResponse {
    pub stack_frames: Vec<StackFrame>,
    pub total_frames: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    pub id: StackFrameId,
    pub name: String,
    pub source: Option<SourceDescriptor>,
    pub line: i64,
    pub column: i64,
    pub end_line: Option<i64>,
    pub end_column: Option<i64>,
    pub module_id: Option<String>,
    pub presentation_hint: Option<StackFramePresentationHint>,
    pub source_reference: Option<i64>,
}

impl Default for StackFrame {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            source: None,
            line: 0,
            column: 0,
            end_line: None,
            end_column: None,
            module_id: None,
            presentation_hint: None,
            source_reference: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFramePresentationHint {
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesRequest {
    pub frame_id: StackFrameId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesResponse {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    pub variables_reference: VariableId,
    pub named_variables: Option<i64>,
    pub indexed_variables: Option<i64>,
    pub expensive: bool,
    pub presentation_hint: Option<VariablePresentationHint>,
    pub source: Option<SourceDescriptor>,
    pub line: Option<i64>,
    pub column: Option<i64>,
    pub end_line: Option<i64>,
    pub end_column: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablePresentationHint {
    pub kind: Option<String>,
    pub attributes: Option<Vec<String>>,
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesRequest {
    pub variables_reference: VariableId,
    pub filter: Option<String>,
    pub start: Option<i64>,
    pub count: Option<i64>,
    pub format: Option<VariableFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableFormat {
    pub hex: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesResponse {
    pub variables: Vec<Variable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_: Option<String>,
    pub variables_reference: VariableId,
    pub named_variables: Option<i64>,
    pub indexed_variables: Option<i64>,
    pub presentation_hint: Option<VariablePresentationHint>,
    pub evaluate_name: Option<String>,
    pub memory_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateRequest {
    pub expression: String,
    pub frame_id: Option<StackFrameId>,
    pub context: Option<String>,
    pub format: Option<VariableFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResponse {
    pub result: String,
    pub type_: Option<String>,
    pub variables_reference: VariableId,
    pub named_variables: Option<i64>,
    pub indexed_variables: Option<i64>,
    pub presentation_hint: Option<VariablePresentationHint>,
    pub evaluate_name: Option<String>,
    pub memory_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepInRequest {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutRequest {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextRequest {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueRequest {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseRequest {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateRequest {
    pub restart: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectRequest {
    pub restart: Option<bool>,
    pub terminate_debuggee: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInTerminalRequest {
    pub kind: Option<String>,
    pub title: Option<String>,
    pub cwd: String,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInTerminalResponse {
    pub process_id: Option<i32>,
    pub shell_process_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEvent {
    pub reason: String,
    pub thread_id: ThreadId,
    pub description: Option<String>,
    pub hit_condition_count: Option<i64>,
    pub text: Option<String>,
    pub all_threads_stopped: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuedEvent {
    pub thread_id: ThreadId,
    pub all_threads_continued: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEvent {
    pub exit_code: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminatedEvent {
    pub restart: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEvent {
    pub reason: String,
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEvent {
    pub category: Option<String>,
    pub output: String,
    pub variables_reference: Option<VariableId>,
    pub source: Option<SourceDescriptor>,
    pub line: Option<i64>,
    pub column: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointEvent {
    pub reason: String,
    pub breakpoint: Breakpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleEvent {
    pub reason: String,
    pub module: Module,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub id: String,
    pub name: String,
    pub path: Option<String>,
    pub is_optimized: Option<bool>,
    pub is_user_code: Option<bool>,
    pub version: Option<String>,
    pub symbol_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSourceEvent {
    pub reason: String,
    pub source: SourceDescriptor,
    pub source_reference: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEvent {
    pub name: String,
    pub start_method: String,
    pub system_process_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressStartEvent {
    pub progress_id: String,
    pub title: String,
    pub request_id: Option<CommandId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdateEvent {
    pub progress_id: String,
    pub message: Option<String>,
    pub percentage: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEndEvent {
    pub progress_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidatedEvent {
    pub areas: Vec<String>,
    pub thread_id: Option<ThreadId>,
    pub stack_frames: Option<Vec<StackFrameId>>,
    pub sources: Option<Vec<SourceDescriptor>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvent {
    pub memory_reference: String,
    pub offset: u64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_launch_request: bool,
    pub supports_attach_request: bool,
    pub supports_set_breakpoints_request: bool,
    pub supports_set_exception_breakpoints_request: bool,
    pub supports_continue_request: bool,
    pub supports_next_request: bool,
    pub supports_step_in_request: bool,
    pub supports_step_out_request: bool,
    pub supports_pause_request: bool,
    pub supports_stack_trace_request: bool,
    pub supports_scopes_request: bool,
    pub supports_variables_request: bool,
    pub supports_evaluate_request: bool,
    pub supports_terminate_request: bool,
    pub supports_disconnect_request: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            supports_configuration_done_request: true,
            supports_launch_request: true,
            supports_attach_request: true,
            supports_set_breakpoints_request: true,
            supports_set_exception_breakpoints_request: true,
            supports_continue_request: true,
            supports_next_request: true,
            supports_step_in_request: true,
            supports_step_out_request: true,
            supports_pause_request: true,
            supports_stack_trace_request: true,
            supports_scopes_request: true,
            supports_variables_request: true,
            supports_evaluate_request: true,
            supports_terminate_request: true,
            supports_disconnect_request: true,
        }
    }
}

impl Message {
    pub fn request(id: CommandId, method: &str, params: serde_json::Value) -> Self {
        Self {
            id: Some(id),
            method: Some(method.to_string()),
            params: Some(params),
            result: None,
            error: None,
            jsonrpc: "2.0".to_string(),
        }
    }

    pub fn response(id: CommandId, result: serde_json::Value) -> Self {
        Self {
            id: Some(id),
            method: None,
            params: None,
            result: Some(result),
            error: None,
            jsonrpc: "2.0".to_string(),
        }
    }

    pub fn error(id: CommandId, code: i64, message: &str) -> Self {
        Self {
            id: Some(id),
            method: None,
            params: None,
            result: None,
            error: Some(ResponseError {
                code,
                message: message.to_string(),
                data: None,
            }),
            jsonrpc: "2.0".to_string(),
        }
    }

    pub fn event(event: &str, params: serde_json::Value) -> Self {
        Self {
            id: None,
            method: Some(event.to_string()),
            params: Some(params),
            result: None,
            error: None,
            jsonrpc: "2.0".to_string(),
        }
    }
}