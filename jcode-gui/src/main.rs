use dioxus::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
mod backend;
#[cfg(target_arch = "wasm32")]
mod backend_web;
mod model;

#[cfg(target_arch = "wasm32")]
use backend_web as backend;

use model::{BackendCommand, GuiModel, RuntimeFeature};

const GUI_CSS: &str = r#"
:root {
  --bg: #0d141b;
  --panel: #15202b;
  --panel-2: #1c2b39;
  --line: #2a3f52;
  --text: #dde8f2;
  --muted: #91a6ba;
  --accent: #f59e0b;
  --ok: #22c55e;
  --warn: #f97316;
  --err: #ef4444;
}
* { box-sizing: border-box; }
body { margin: 0; background: radial-gradient(circle at top right, #1b2e3d, var(--bg)); color: var(--text); font-family: "Iosevka Aile", "JetBrains Mono", ui-monospace, monospace; }
.app { display: grid; grid-template-columns: 320px 1fr 320px; gap: 12px; height: 100vh; padding: 12px; }
.panel { border: 1px solid var(--line); border-radius: 12px; background: color-mix(in srgb, var(--panel) 88%, black); overflow: hidden; min-height: 0; display: flex; flex-direction: column; }
.panel-head { padding: 10px 12px; border-bottom: 1px solid var(--line); background: color-mix(in srgb, var(--panel-2) 80%, black); font-weight: 700; }
.panel-body { padding: 10px 12px; overflow: auto; display: flex; flex-direction: column; gap: 10px; }
.kv { display: grid; grid-template-columns: 1fr; gap: 3px; font-size: 12px; }
.kv .k { color: var(--muted); font-size: 11px; }
.row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
button { border: 1px solid var(--line); background: #142130; color: var(--text); padding: 6px 8px; border-radius: 8px; cursor: pointer; font-family: inherit; font-size: 12px; }
button:hover { border-color: var(--accent); }
button.primary { background: #24384c; border-color: #385671; }
button.warn { border-color: var(--warn); color: #ffd2b2; }
button.danger { border-color: var(--err); color: #ffc2c2; }
button.on { border-color: var(--ok); color: #b8f3ca; }
input, textarea { width: 100%; background: #0f1a26; color: var(--text); border: 1px solid var(--line); border-radius: 8px; padding: 8px; font: inherit; font-size: 12px; }
textarea { min-height: 90px; resize: vertical; }
.messages { flex: 1; overflow: auto; padding: 10px 12px; display: flex; flex-direction: column; gap: 10px; }
.msg { border: 1px solid var(--line); border-radius: 10px; background: #12202e; }
.msg-head { font-size: 11px; color: var(--muted); padding: 7px 10px; border-bottom: 1px solid var(--line); }
.msg-body { font-size: 12px; white-space: pre-wrap; padding: 8px 10px; line-height: 1.38; }
.msg.user { border-color: #2d4f76; }
.msg.assistant { border-color: #325f4e; }
.msg.error { border-color: var(--err); }
.msg.tool { border-color: #6a5b2e; }
.stream { border-style: dashed; }
.composer { border-top: 1px solid var(--line); padding: 10px 12px; display: grid; gap: 8px; }
.badge { border: 1px solid var(--line); border-radius: 999px; padding: 2px 8px; font-size: 11px; color: var(--muted); }
.badge.ok { border-color: var(--ok); color: #b8f3ca; }
.badge.err { border-color: var(--err); color: #ffc2c2; }
.logline { font-size: 11px; white-space: pre-wrap; border-bottom: 1px dashed #203243; padding: 4px 0; }
.grid-meta { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.small { font-size: 11px; color: var(--muted); }
@media (max-width: 1200px) {
  .app { grid-template-columns: 1fr; grid-auto-rows: minmax(220px, auto); height: auto; }
  body { min-height: 100vh; }
}
"#;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        dioxus::LaunchBuilder::desktop().launch(app);
    }

    #[cfg(target_arch = "wasm32")]
    {
        dioxus::LaunchBuilder::web().launch(app);
    }
}

fn app() -> Element {
    let mut model = use_signal(GuiModel::default);
    let backend = use_hook(backend::BackendBridge::spawn);

    {
        let backend = backend.clone();
        let mut model = model;
        use_future(move || {
            let backend = backend.clone();
            async move {
                while let Some(event) = backend.next_event().await {
                    model.write().apply_backend_event(event);
                }
            }
        });
    }

    let snapshot = model.read().clone();

    rsx! {
        document::Title { "jcode GUI" }
        style { "{GUI_CSS}" }

        div { class: "app",
            div { class: "panel",
                div { class: "panel-head", "Session / Controls" }
                div { class: "panel-body",
                    div { class: "row",
                        if snapshot.connected {
                            span { class: "badge ok", "connected" }
                        } else {
                            span { class: "badge err", "disconnected" }
                        }
                        if let Some(session_id) = snapshot.session_id.clone() {
                            span { class: "badge", "{session_id}" }
                        }
                    }

                    div { class: "grid-meta",
                        div { class: "kv",
                            div { class: "k", "provider" }
                            div { "{snapshot.provider_name}" }
                        }
                        div { class: "kv",
                            div { class: "k", "model" }
                            div { "{snapshot.provider_model}" }
                        }
                        div { class: "kv",
                            div { class: "k", "turn tokens" }
                            div { "in {snapshot.turn_input_tokens} / out {snapshot.turn_output_tokens}" }
                        }
                        div { class: "kv",
                            div { class: "k", "session tokens" }
                            div { "in {snapshot.total_input_tokens} / out {snapshot.total_output_tokens}" }
                        }
                    }

                    if let Some(reason) = snapshot.connection_reason.clone() {
                        div { class: "small", "{reason}" }
                    }

                    div { class: "row",
                        button {
                            class: if snapshot.memory_enabled { "on" } else { "" },
                            onclick: {
                                let backend = backend.clone();
                                move |_| {
                                    let enabled = {
                                        let mut state = model.write();
                                        state.memory_enabled = !state.memory_enabled;
                                        state.memory_enabled
                                    };
                                    backend.send(BackendCommand::SetFeature {
                                        feature: RuntimeFeature::Memory,
                                        enabled,
                                    });
                                }
                            },
                            "Memory: " { if snapshot.memory_enabled { "on" } else { "off" } }
                        }

                        button {
                            class: if snapshot.swarm_enabled { "on" } else { "" },
                            onclick: {
                                let backend = backend.clone();
                                move |_| {
                                    let enabled = {
                                        let mut state = model.write();
                                        state.swarm_enabled = !state.swarm_enabled;
                                        state.swarm_enabled
                                    };
                                    backend.send(BackendCommand::SetFeature {
                                        feature: RuntimeFeature::Swarm,
                                        enabled,
                                    });
                                }
                            },
                            "Swarm: " { if snapshot.swarm_enabled { "on" } else { "off" } }
                        }
                    }

                    div { class: "row",
                        button {
                            class: "warn",
                            onclick: {
                                let backend = backend.clone();
                                move |_| backend.send(BackendCommand::Cancel)
                            },
                            "Cancel"
                        }
                        button {
                            class: "danger",
                            onclick: {
                                let backend = backend.clone();
                                move |_| backend.send(BackendCommand::Clear)
                            },
                            "Clear"
                        }
                        button {
                            onclick: {
                                let backend = backend.clone();
                                move |_| backend.send(BackendCommand::Reload)
                            },
                            "Reload"
                        }
                    }

                    div { class: "row",
                        button {
                            onclick: {
                                let backend = backend.clone();
                                move |_| backend.send(BackendCommand::CycleModel(-1))
                            },
                            "Model -"
                        }
                        button {
                            onclick: {
                                let backend = backend.clone();
                                move |_| backend.send(BackendCommand::CycleModel(1))
                            },
                            "Model +"
                        }
                    }

                    div { class: "kv",
                        div { class: "k", "set model" }
                        input {
                            value: snapshot.model_input.clone(),
                            oninput: move |evt| {
                                model.write().model_input = evt.value();
                            }
                        }
                        button {
                            onclick: {
                                let backend = backend.clone();
                                move |_| {
                                    let value = {
                                        let mut state = model.write();
                                        let value = state.model_input.trim().to_string();
                                        state.model_input.clear();
                                        value
                                    };
                                    if !value.is_empty() {
                                        backend.send(BackendCommand::SetModel(value));
                                    }
                                }
                            },
                            "Apply model"
                        }
                    }

                    div { class: "kv",
                        div { class: "k", "resume session" }
                        input {
                            value: snapshot.resume_session_input.clone(),
                            oninput: move |evt| {
                                model.write().resume_session_input = evt.value();
                            }
                        }
                        button {
                            onclick: {
                                let backend = backend.clone();
                                move |_| {
                                    let value = {
                                        let mut state = model.write();
                                        let value = state.resume_session_input.trim().to_string();
                                        state.resume_session_input.clear();
                                        value
                                    };
                                    if !value.is_empty() {
                                        backend.send(BackendCommand::ResumeSession(value));
                                    }
                                }
                            },
                            "Resume"
                        }
                    }

                    if !snapshot.available_models.is_empty() {
                        div { class: "kv",
                            div { class: "k", "available models" }
                            div { class: "row",
                                for model_name in snapshot.available_models.iter().take(12) {
                                    button {
                                        key: "{model_name}",
                                        onclick: {
                                            let backend = backend.clone();
                                            let model_name = model_name.clone();
                                            move |_| backend.send(BackendCommand::SetModel(model_name.clone()))
                                        },
                                        "{model_name}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "panel",
                div { class: "panel-head", "Conversation" }
                div { class: "messages",
                    for (idx, message) in snapshot.messages.iter().enumerate() {
                        article {
                            key: "{idx}",
                            class: "msg {message.role}",
                            div { class: "msg-head",
                                "{message.role}"
                                if !message.tool_calls.is_empty() {
                                    "  tools: "
                                    {message.tool_calls.join(", ")}
                                }
                            }
                            div { class: "msg-body", "{message.content}" }
                        }
                    }

                    if !snapshot.streaming_text.is_empty() {
                        article {
                            class: "msg assistant stream",
                            div { class: "msg-head", "assistant (streaming)" }
                            div { class: "msg-body", "{snapshot.streaming_text}" }
                        }
                    }
                }

                div { class: "composer",
                    textarea {
                        value: snapshot.composer.clone(),
                        oninput: move |evt| {
                            model.write().composer = evt.value();
                        }
                    }

                    div { class: "row",
                        button {
                            class: "primary",
                            onclick: {
                                let backend = backend.clone();
                                move |_| {
                                    let value = {
                                        let mut state = model.write();
                                        let value = state.composer.trim().to_string();
                                        state.composer.clear();
                                        value
                                    };

                                    if !value.is_empty() {
                                        backend.send(BackendCommand::SendMessage(value));
                                    }
                                }
                            },
                            "Send"
                        }
                    }

                    div { class: "kv",
                        div { class: "k", "soft interrupt" }
                        input {
                            value: snapshot.soft_interrupt.clone(),
                            oninput: move |evt| {
                                model.write().soft_interrupt = evt.value();
                            }
                        }
                        div { class: "row",
                            button {
                                onclick: {
                                    let backend = backend.clone();
                                    move |_| {
                                        let value = {
                                            let mut state = model.write();
                                            let value = state.soft_interrupt.trim().to_string();
                                            state.soft_interrupt.clear();
                                            value
                                        };

                                        if !value.is_empty() {
                                            backend.send(BackendCommand::SoftInterrupt {
                                                content: value,
                                                urgent: false,
                                            });
                                        }
                                    }
                                },
                                "Inject"
                            }
                            button {
                                class: "warn",
                                onclick: {
                                    let backend = backend.clone();
                                    move |_| {
                                        let value = {
                                            let mut state = model.write();
                                            let value = state.soft_interrupt.trim().to_string();
                                            state.soft_interrupt.clear();
                                            value
                                        };

                                        if !value.is_empty() {
                                            backend.send(BackendCommand::SoftInterrupt {
                                                content: value,
                                                urgent: true,
                                            });
                                        }
                                    }
                                },
                                "Inject urgent"
                            }
                        }
                    }
                }
            }

            div { class: "panel",
                div { class: "panel-head", "Activity / Context" }
                div { class: "panel-body",
                    if let Some(version) = snapshot.server_version.clone() {
                        div { class: "kv",
                            div { class: "k", "server version" }
                            div { "{version}" }
                        }
                    }

                    if let Some(name) = snapshot.server_name.clone() {
                        div { class: "kv",
                            div { class: "k", "server" }
                            div {
                                if let Some(icon) = snapshot.server_icon.clone() {
                                    "{icon} "
                                }
                                "{name}"
                            }
                        }
                    }

                    if let Some(provider) = snapshot.upstream_provider.clone() {
                        div { class: "kv",
                            div { class: "k", "upstream provider" }
                            div { "{provider}" }
                        }
                    }

                    if let Some(cache_read) = snapshot.cache_read_input {
                        div { class: "kv",
                            div { class: "k", "cache read input" }
                            div { "{cache_read}" }
                        }
                    }

                    if let Some(cache_creation) = snapshot.cache_creation_input {
                        div { class: "kv",
                            div { class: "k", "cache creation input" }
                            div { "{cache_creation}" }
                        }
                    }

                    if !snapshot.mcp_servers.is_empty() {
                        div { class: "kv",
                            div { class: "k", "mcp servers" }
                            div { {snapshot.mcp_servers.join(", ")} }
                        }
                    }

                    if !snapshot.skills.is_empty() {
                        div { class: "kv",
                            div { class: "k", "skills" }
                            div { {snapshot.skills.join(", ")} }
                        }
                    }

                    if let Some(error) = snapshot.last_error.clone() {
                        div { class: "kv",
                            div { class: "k", "last error" }
                            div { "{error}" }
                        }
                    }

                    div { class: "kv",
                        div { class: "k", "activity" }
                        for (idx, line) in snapshot.activity_log.iter().enumerate().rev().take(80) {
                            div {
                                key: "log-{idx}",
                                class: "logline",
                                "{line}"
                            }
                        }
                    }
                }
            }
        }
    }
}
