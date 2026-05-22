//! DAP 调试会话管理模块
//! 
//! 管理调试会话的生命周期、线程状态、栈帧信息等

use super::protocol::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugSessionState {
    Initialized,
    Configured,
    Running,
    Paused,
    Stopped,
    Disconnected,
}

#[derive(Debug)]
pub struct StackFrameData {
    pub frame: StackFrame,
    pub scopes: Vec<Scope>,
    pub variables: HashMap<String, Variable>,
}

#[derive(Debug)]
pub struct ThreadState {
    pub id: ThreadId,
    pub name: String,
    pub state: ThreadStateEnum,
    pub stack_frames: Vec<StackFrameId>,
    pub current_frame_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadStateEnum {
    Running,
    Paused,
    Stopped,
}

pub struct DebugSession {
    pub id: String,
    pub state: DebugSessionState,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    
    pub threads: HashMap<ThreadId, ThreadState>,
    pub frames: HashMap<StackFrameId, StackFrameData>,
    pub breakpoints: HashMap<BreakpointId, Breakpoint>,
    pub source_breakpoints: HashMap<String, Vec<BreakpointId>>,
    
    pub variables_cache: HashMap<VariableId, Vec<Variable>>,
    pub next_variable_id: VariableId,
    
    pub capabilities: Capabilities,
    pub initialized: bool,
    pub configuration_done: bool,
}

impl DebugSession {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            state: DebugSessionState::Initialized,
            program: None,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            threads: HashMap::new(),
            frames: HashMap::new(),
            breakpoints: HashMap::new(),
            source_breakpoints: HashMap::new(),
            variables_cache: HashMap::new(),
            next_variable_id: 1,
            capabilities: Capabilities::default(),
            initialized: false,
            configuration_done: false,
        }
    }

    pub fn start(&mut self, program: &str, args: &[String], cwd: Option<&str>) {
        self.program = Some(program.to_string());
        self.args = args.to_vec();
        self.cwd = cwd.map(|s| s.to_string());
        self.state = DebugSessionState::Running;
        
        let main_thread = ThreadState {
            id: 1,
            name: "Main Thread".to_string(),
            state: ThreadStateEnum::Running,
            stack_frames: Vec::new(),
            current_frame_index: 0,
        };
        self.threads.insert(1, main_thread);
    }

    pub fn pause(&mut self, thread_id: ThreadId) -> Option<&ThreadState> {
        {
            let thread = self.threads.get_mut(&thread_id)?;
            thread.state = ThreadStateEnum::Paused;
            self.state = DebugSessionState::Paused;
        }
        self.generate_stack_frames(thread_id);
        self.threads.get(&thread_id)
    }

    pub fn continue_execution(&mut self, thread_id: ThreadId) -> Option<&ThreadState> {
        let all_running = self.threads.values().all(|t| t.state == ThreadStateEnum::Running);
        if all_running {
            self.state = DebugSessionState::Running;
        }
        
        let thread = self.threads.get_mut(&thread_id)?;
        thread.state = ThreadStateEnum::Running;
        Some(thread)
    }

    pub fn step_in(&mut self, thread_id: ThreadId) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            if thread.state == ThreadStateEnum::Paused {
                if thread.current_frame_index < thread.stack_frames.len() {
                    thread.current_frame_index += 1;
                }
            }
        }
    }

    pub fn step_out(&mut self, thread_id: ThreadId) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            if thread.state == ThreadStateEnum::Paused {
                if thread.current_frame_index > 0 {
                    thread.current_frame_index -= 1;
                }
            }
        }
    }

    pub fn next(&mut self, thread_id: ThreadId) {
        if let Some(thread) = self.threads.get_mut(&thread_id) {
            if thread.state == ThreadStateEnum::Paused {
                if thread.current_frame_index < thread.stack_frames.len() {
                    thread.current_frame_index += 1;
                }
            }
        }
    }

    pub fn terminate(&mut self) {
        self.state = DebugSessionState::Stopped;
        for thread in self.threads.values_mut() {
            thread.state = ThreadStateEnum::Stopped;
        }
    }

    pub fn disconnect(&mut self) {
        self.state = DebugSessionState::Disconnected;
    }

    pub fn add_breakpoint(&mut self, source: &SourceDescriptor, line: i64) -> Breakpoint {
        let breakpoint_id = format!("bp-{}", self.breakpoints.len() + 1);
        let breakpoint = Breakpoint {
            id: Some(breakpoint_id.clone()),
            verified: true,
            line: Some(line),
            column: None,
            source: Some(source.clone()),
            message: None,
            condition: None,
            hit_condition: None,
            log_message: None,
            disabled: Some(false),
            pending: None,
        };
        
        self.breakpoints.insert(breakpoint_id.clone(), breakpoint.clone());
        
        let path = source.path.clone().unwrap_or_default();
        self.source_breakpoints
            .entry(path)
            .or_insert_with(Vec::new)
            .push(breakpoint_id);
        
        breakpoint
    }

    pub fn remove_breakpoint(&mut self, breakpoint_id: &BreakpointId) -> bool {
        if let Some(breakpoint) = self.breakpoints.remove(breakpoint_id) {
            if let Some(source) = &breakpoint.source {
                if let Some(path) = &source.path {
                    if let Some(bps) = self.source_breakpoints.get_mut(path) {
                        bps.retain(|id| id != breakpoint_id);
                    }
                }
            }
            true
        } else {
            false
        }
    }

    pub fn clear_breakpoints(&mut self, source_path: &str) {
        if let Some(breakpoint_ids) = self.source_breakpoints.remove(source_path) {
            for id in breakpoint_ids {
                self.breakpoints.remove(&id);
            }
        }
    }

    pub fn set_exception_breakpoints(&mut self, filters: &[String]) {
        for filter in filters {
        }
    }

    pub fn get_threads(&self) -> Vec<Thread> {
        self.threads
            .values()
            .map(|t| Thread {
                id: t.id,
                name: t.name.clone(),
            })
            .collect()
    }

    pub fn get_stack_frames(&self, thread_id: ThreadId, start_frame: i64, levels: i64) -> Option<Vec<StackFrame>> {
        let thread = self.threads.get(&thread_id)?;
        let start = start_frame as usize;
        let end = start + levels as usize;
        
        Some(thread.stack_frames[start..end.min(thread.stack_frames.len())]
            .iter()
            .filter_map(|&frame_id| self.frames.get(&frame_id).map(|d| d.frame.clone()))
            .collect())
    }

    pub fn get_scopes(&self, frame_id: StackFrameId) -> Option<Vec<Scope>> {
        self.frames.get(&frame_id).map(|d| d.scopes.clone())
    }

    pub fn get_variables(&self, variables_reference: VariableId) -> Option<Vec<Variable>> {
        self.variables_cache.get(&variables_reference).cloned()
    }

    pub fn evaluate(&self, expression: &str, frame_id: Option<StackFrameId>) -> EvaluateResponse {
        EvaluateResponse {
            result: format!("Evaluated: {}", expression),
            type_: Some("string".to_string()),
            variables_reference: 0,
            named_variables: None,
            indexed_variables: None,
            presentation_hint: None,
            evaluate_name: None,
            memory_reference: None,
        }
    }

    fn generate_stack_frames(&mut self, thread_id: ThreadId) {
        if !self.threads.contains_key(&thread_id) {
            return;
        }
        
        let frames = vec![
            StackFrame {
                id: 1,
                name: "main".to_string(),
                source: Some(SourceDescriptor {
                    name: Some("main.rs".to_string()),
                    path: self.cwd.clone().map(|c| format!("{}/src/main.rs", c)),
                    source_reference: None,
                    origin: None,
                    presentation_hint: None,
                }),
                line: 10,
                column: 5,
                end_line: None,
                end_column: None,
                module_id: None,
                presentation_hint: None,
                source_reference: None,
            },
            StackFrame {
                id: 2,
                name: "process_input".to_string(),
                source: Some(SourceDescriptor {
                    name: Some("utils.rs".to_string()),
                    path: self.cwd.clone().map(|c| format!("{}/src/utils.rs", c)),
                    source_reference: None,
                    origin: None,
                    presentation_hint: None,
                }),
                line: 25,
                column: 10,
                end_line: None,
                end_column: None,
                module_id: None,
                presentation_hint: None,
                source_reference: None,
            },
            StackFrame {
                id: 3,
                name: "parse_command".to_string(),
                source: Some(SourceDescriptor {
                    name: Some("parser.rs".to_string()),
                    path: self.cwd.clone().map(|c| format!("{}/src/parser.rs", c)),
                    source_reference: None,
                    origin: None,
                    presentation_hint: None,
                }),
                line: 45,
                column: 20,
                end_line: None,
                end_column: None,
                module_id: None,
                presentation_hint: None,
                source_reference: None,
            },
        ];
        
        let mut frame_ids = Vec::new();
        let mut frame_datas: Vec<(StackFrame, Vec<Scope>, HashMap<String, Variable>)> = Vec::new();
        for (idx, frame) in frames.into_iter().enumerate() {
            let frame_id = (idx + 1) as StackFrameId;
            frame_ids.push(frame_id);
            
            let scopes = self.generate_scopes(frame_id);
            let variables = self.generate_variables();
            
            frame_datas.push((frame, scopes, variables));
        }
        
        let thread = self.threads.get_mut(&thread_id);
        let thread = thread.unwrap();
        
        for (idx, (frame, scopes, variables)) in frame_datas.into_iter().enumerate() {
            let frame_id = (idx + 1) as StackFrameId;
            self.frames.insert(frame_id, StackFrameData {
                frame,
                scopes: scopes.clone(),
                variables,
            });
        }
        
        thread.stack_frames = frame_ids;
        thread.current_frame_index = 0;
    }

    fn generate_scopes(&mut self, frame_id: StackFrameId) -> Vec<Scope> {
        let local_vars_id = self.next_variable_id;
        self.next_variable_id += 1;
        let args_id = self.next_variable_id;
        self.next_variable_id += 1;
        
        let local_vars = vec![
            Variable {
                name: "x".to_string(),
                value: "42".to_string(),
                type_: Some("i32".to_string()),
                variables_reference: 0,
                named_variables: None,
                indexed_variables: None,
                presentation_hint: None,
                evaluate_name: None,
                memory_reference: None,
            },
            Variable {
                name: "result".to_string(),
                value: "\"hello world\"".to_string(),
                type_: Some("String".to_string()),
                variables_reference: 0,
                named_variables: None,
                indexed_variables: None,
                presentation_hint: None,
                evaluate_name: None,
                memory_reference: None,
            },
        ];
        self.variables_cache.insert(local_vars_id, local_vars);
        
        vec![
            Scope {
                name: "Locals".to_string(),
                variables_reference: local_vars_id,
                named_variables: Some(2),
                indexed_variables: None,
                expensive: false,
                presentation_hint: None,
                source: None,
                line: None,
                column: None,
                end_line: None,
                end_column: None,
            },
            Scope {
                name: "Arguments".to_string(),
                variables_reference: args_id,
                named_variables: Some(0),
                indexed_variables: None,
                expensive: false,
                presentation_hint: None,
                source: None,
                line: None,
                column: None,
                end_line: None,
                end_column: None,
            },
        ]
    }

    fn generate_variables(&self) -> HashMap<String, Variable> {
        HashMap::new()
    }
}

pub struct DebugSessionManager {
    sessions: HashMap<String, Arc<Mutex<DebugSession>>>,
    next_session_id: u64,
}

impl DebugSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_session_id: 1,
        }
    }

    pub fn create_session(&mut self) -> String {
        let session_id = format!("session-{}", self.next_session_id);
        self.next_session_id += 1;
        
        let session = DebugSession::new(&session_id);
        self.sessions.insert(session_id.clone(), Arc::new(Mutex::new(session)));
        
        session_id
    }

    pub fn get_session(&self, session_id: &str) -> Option<Arc<Mutex<DebugSession>>> {
        self.sessions.get(session_id).cloned()
    }

    pub fn remove_session(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    pub fn list_sessions(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }
}