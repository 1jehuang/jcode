//! DAP 调试适配器核心模块
//! 
//! 实现 Debug Adapter Protocol 的核心处理逻辑

use super::protocol::*;
use super::session::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{log};

pub struct DebugAdapter {
    session_manager: Arc<Mutex<DebugSessionManager>>,
    command_sender: mpsc::Sender<AdapterCommand>,
    event_sender: mpsc::Sender<AdapterEvent>,
}

pub enum AdapterCommand {
    Initialize(CommandId, InitializeRequest),
    Launch(CommandId, LaunchRequest),
    Attach(CommandId, AttachRequest),
    ConfigurationDone(CommandId),
    SetBreakpoints(CommandId, SetBreakpointsRequest),
    SetExceptionBreakpoints(CommandId, SetExceptionBreakpointsRequest),
    Threads(CommandId),
    StackTrace(CommandId, StackTraceRequest),
    Scopes(CommandId, ScopesRequest),
    Variables(CommandId, VariablesRequest),
    Evaluate(CommandId, EvaluateRequest),
    StepIn(CommandId, StepInRequest),
    StepOut(CommandId, StepOutRequest),
    Next(CommandId, NextRequest),
    Continue(CommandId, ContinueRequest),
    Pause(CommandId, PauseRequest),
    Terminate(CommandId, TerminateRequest),
    Disconnect(CommandId, DisconnectRequest),
    Shutdown,
}

pub enum AdapterEvent {
    Stopped(StoppedEvent),
    Continued(ContinuedEvent),
    Exited(ExitedEvent),
    Terminated(TerminatedEvent),
    Thread(ThreadEvent),
    Output(OutputEvent),
    Breakpoint(BreakpointEvent),
}

impl DebugAdapter {
    pub async fn new() -> Self {
        let (command_sender, command_receiver) = mpsc::channel(100);
        let (event_sender, _event_receiver) = mpsc::channel(100);
        
        let session_manager = Arc::new(Mutex::new(DebugSessionManager::new()));
        
        let sm_clone = session_manager.clone();
        let es_clone = event_sender.clone();
        
        tokio::spawn(async move {
            Self::command_handler(sm_clone, command_receiver, es_clone).await;
        });
        
        Self {
            session_manager,
            command_sender,
            event_sender,
        }
    }

    pub async fn start_server(&self, addr: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("DAP server listening on {}", addr);
        
        loop {
            let (stream, _) = listener.accept().await?;
            let command_sender = self.command_sender.clone();
            let (_, event_receiver) = mpsc::channel(100);
            
            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, command_sender, event_receiver).await {
                    log::error!("Connection error: {}", e);
                }
            });
        }
    }

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        command_sender: mpsc::Sender<AdapterCommand>,
        mut event_receiver: mpsc::Receiver<AdapterEvent>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        
        let mut buf = String::new();
        let mut current_session_id = None;
        
        loop {
            tokio::select! {
                result = reader.read_line(&mut buf) => {
                    if result? == 0 {
                        break;
                    }
                    
                    if let Ok(message) = serde_json::from_str::<Message>(&buf) {
                        let response = Self::process_message(&message, &mut current_session_id, &command_sender).await;
                        if let Some(response) = response {
                            let response_json = serde_json::to_string(&response)?;
                            writer.write_all(response_json.as_bytes()).await?;
                            writer.flush().await?;
                        }
                    }
                    
                    buf.clear();
                }
                event = event_receiver.recv() => {
                    if let Some(event) = event {
                        let event_message = Self::event_to_message(event);
                        let event_json = serde_json::to_string(&event_message)?;
                        writer.write_all(event_json.as_bytes()).await?;
                        writer.flush().await?;
                    }
                }
            }
        }
        
        Ok(())
    }

    async fn process_message(
        message: &Message,
        current_session_id: &mut Option<String>,
        command_sender: &mpsc::Sender<AdapterCommand>,
    ) -> Option<Message> {
        let id = message.id?;
        let method = message.method.as_deref()?;
        
        match method {
            "initialize" => {
                let params: InitializeRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Initialize(id, params)).await.ok()?;
                let response = InitializeResponse {
                    supports_configuration_done_request: Some(true),
                    supports_launch_request: Some(true),
                    supports_attach_request: Some(true),
                    supports_restart_request: Some(true),
                    supports_set_breakpoints_request: Some(true),
                    supports_set_exception_breakpoints_request: Some(true),
                    supports_delayed_stack_trace_loading: Some(true),
                    supports_data_breakpoints: Some(false),
                    supports_conditional_breakpoints: Some(true),
                    supports_log_breakpoints: Some(true),
                    supports_evaluate_for_hovers: Some(true),
                    exception_breakpoint_filters: Some(vec![
                        ExceptionBreakpointFilter {
                            filter: "all".to_string(),
                            label: "All Exceptions".to_string(),
                            default: Some(true),
                            description: Some("Break on all exceptions".to_string()),
                        },
                    ]),
                    ..Default::default()
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "launch" => {
                let params: LaunchRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Launch(id, params)).await.ok()?;
                *current_session_id = Some("session-1".to_string());
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "attach" => {
                let params: AttachRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Attach(id, params)).await.ok()?;
                *current_session_id = Some("session-1".to_string());
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "configurationDone" => {
                command_sender.send(AdapterCommand::ConfigurationDone(id)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "setBreakpoints" => {
                let params: SetBreakpointsRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::SetBreakpoints(id, params)).await.ok()?;
                let response = SetBreakpointsResponse {
                    breakpoints: Vec::new(),
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "setExceptionBreakpoints" => {
                let params: SetExceptionBreakpointsRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::SetExceptionBreakpoints(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "threads" => {
                command_sender.send(AdapterCommand::Threads(id)).await.ok()?;
                let response = ThreadsResponse {
                    threads: vec![Thread {
                        id: 1,
                        name: "Main Thread".to_string(),
                    }],
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "stackTrace" => {
                let params: StackTraceRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::StackTrace(id, params)).await.ok()?;
                let response = StackTraceResponse {
                    stack_frames: vec![
                        StackFrame {
                            id: 1,
                            name: "main".to_string(),
                            source: None,
                            line: 10,
                            column: 5,
                            ..Default::default()
                        },
                    ],
                    total_frames: Some(1),
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "scopes" => {
                let params: ScopesRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Scopes(id, params)).await.ok()?;
                let response = ScopesResponse {
                    scopes: vec![
                        Scope {
                            name: "Locals".to_string(),
                            variables_reference: 1,
                            named_variables: Some(2),
                            indexed_variables: None,
                            expensive: false,
                            ..Default::default()
                        },
                    ],
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "variables" => {
                let params: VariablesRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Variables(id, params)).await.ok()?;
                let response = VariablesResponse {
                    variables: vec![
                        Variable {
                            name: "x".to_string(),
                            value: "42".to_string(),
                            type_: Some("i32".to_string()),
                            variables_reference: 0,
                            ..Default::default()
                        },
                    ],
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "evaluate" => {
                let params: EvaluateRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Evaluate(id, params)).await.ok()?;
                let response = EvaluateResponse {
                    result: "42".to_string(),
                    type_: Some("i32".to_string()),
                    variables_reference: 0,
                    ..Default::default()
                };
                Some(Message::response(id, serde_json::to_value(response).ok()?))
            }
            
            "stepIn" => {
                let params: StepInRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::StepIn(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "stepOut" => {
                let params: StepOutRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::StepOut(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "next" => {
                let params: NextRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Next(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "continue" => {
                let params: ContinueRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Continue(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "pause" => {
                let params: PauseRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Pause(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "terminate" => {
                let params: TerminateRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Terminate(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            "disconnect" => {
                let params: DisconnectRequest = serde_json::from_value(message.params.clone()?).ok()?;
                command_sender.send(AdapterCommand::Disconnect(id, params)).await.ok()?;
                Some(Message::response(id, serde_json::json!({})))
            }
            
            _ => {
                log::warn!("Unknown method: {}", method);
                Some(Message::error(id, -32601, "Method not found"))
            }
        }
    }

    async fn command_handler(
        session_manager: Arc<Mutex<DebugSessionManager>>,
        mut receiver: mpsc::Receiver<AdapterCommand>,
        event_sender: mpsc::Sender<AdapterEvent>,
    ) {
        while let Some(command) = receiver.recv().await {
            match command {
                AdapterCommand::Initialize(_id, _) => {
                    log::info!("Initialize request received");
                }
                AdapterCommand::Launch(_id, params) => {
                    log::info!("Launch request: {:?}", params);
                    let mut sm = session_manager.lock().unwrap();
                    let session_id = sm.create_session();
                    if let Some(session) = sm.get_session(&session_id) {
                        let mut s = session.lock().unwrap();
                        s.start(
                            params.program.as_deref().unwrap_or(""),
                            params.args.as_ref().unwrap_or(&Vec::new()),
                            params.cwd.as_deref(),
                        );
                    }
                }
                AdapterCommand::Attach(_id, params) => {
                    log::info!("Attach request: {:?}", params);
                }
                AdapterCommand::SetBreakpoints(_id, params) => {
                    log::info!("Set breakpoints: {:?}", params);
                    let sm = session_manager.lock().unwrap();
                    if let Some(session) = sm.get_session("session-1") {
                        let mut s = session.lock().unwrap();
                        if let Some(breakpoints) = params.breakpoints {
                            for bp in breakpoints {
                                s.add_breakpoint(&params.source, bp.line);
                            }
                        }
                    }
                }
                AdapterCommand::Threads(_id) => {
                    log::info!("Threads request");
                }
                AdapterCommand::StackTrace(_id, params) => {
                    log::info!("Stack trace request: {:?}", params);
                }
                AdapterCommand::Pause(_id, params) => {
                    log::info!("Pause request: {:?}", params);
                    let thread_id = params.thread_id;
                    
                    {
                        let sm = session_manager.lock().unwrap();
                        if let Some(session) = sm.get_session("session-1") {
                            let mut s = session.lock().unwrap();
                            s.pause(params.thread_id);
                        }
                    }
                    
                    event_sender.send(AdapterEvent::Stopped(StoppedEvent {
                        reason: "pause".to_string(),
                        thread_id,
                        description: None,
                        hit_condition_count: None,
                        text: None,
                        all_threads_stopped: Some(true),
                    })).await.ok();
                }
                AdapterCommand::Continue(_id, params) => {
                    log::info!("Continue request: {:?}", params);
                    let thread_id = params.thread_id;
                    
                    {
                        let sm = session_manager.lock().unwrap();
                        if let Some(session) = sm.get_session("session-1") {
                            let mut s = session.lock().unwrap();
                            s.continue_execution(params.thread_id);
                        }
                    }
                    
                    event_sender.send(AdapterEvent::Continued(ContinuedEvent {
                        thread_id,
                        all_threads_continued: Some(true),
                    })).await.ok();
                }
                AdapterCommand::Terminate(_id, params) => {
                    log::info!("Terminate request: {:?}", params);
                    let restart = params.restart;
                    
                    {
                        let sm = session_manager.lock().unwrap();
                        if let Some(session) = sm.get_session("session-1") {
                            let mut s = session.lock().unwrap();
                            s.terminate();
                        }
                    }
                    
                    event_sender.send(AdapterEvent::Terminated(TerminatedEvent {
                        restart,
                    })).await.ok();
                }
                AdapterCommand::Disconnect(_id, params) => {
                    log::info!("Disconnect request: {:?}", params);
                    let sm = session_manager.lock().unwrap();
                    if let Some(session) = sm.get_session("session-1") {
                        let mut s = session.lock().unwrap();
                        s.disconnect();
                    }
                }
                _ => {}
            }
        }
    }

    fn event_to_message(event: AdapterEvent) -> Message {
        match event {
            AdapterEvent::Stopped(e) => {
                Message::event("stopped", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Continued(e) => {
                Message::event("continued", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Exited(e) => {
                Message::event("exited", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Terminated(e) => {
                Message::event("terminated", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Thread(e) => {
                Message::event("thread", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Output(e) => {
                Message::event("output", serde_json::to_value(e).unwrap())
            }
            AdapterEvent::Breakpoint(e) => {
                Message::event("breakpoint", serde_json::to_value(e).unwrap())
            }
        }
    }
}