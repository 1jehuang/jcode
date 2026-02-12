use dioxus::prelude::*;

#[cfg(all(not(target_arch = "wasm32"), unix))]
mod backend;
#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
mod backend_desktop_stub;
#[cfg(target_arch = "wasm32")]
mod backend_web;
mod model;

#[cfg(all(not(target_arch = "wasm32"), unix))]
use backend as backend_impl;
#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
use backend_desktop_stub as backend_impl;
#[cfg(target_arch = "wasm32")]
use backend_web as backend_impl;

use model::{BackendCommand, GuiModel, RuntimeFeature};

const GUI_CSS: &str = r#"
:root {
  --bg: #000000;
  --bg-grad: #000000;
  --surface: #040404;
  --surface-2: #080808;
  --line: #1b1b1b;
  --text: #f3f3f3;
  --muted: #8a8a8a;
  --accent: #22c55e;
  --ok: #22c55e;
  --warn: #f59e0b;
  --err: #ef4444;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font-family: "Iosevka Aile", "JetBrains Mono", ui-monospace, monospace;
}
.shell {
  display: flex;
  justify-content: center;
  width: 100%;
  height: 100vh;
  padding: 0 14px;
  background: #000000;
}
.main-col {
  width: min(980px, 100%);
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-left: 1px solid color-mix(in srgb, var(--line) 80%, transparent);
  border-right: 1px solid color-mix(in srgb, var(--line) 80%, transparent);
  background: #000000;
}
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 12px 14px;
  border-bottom: 1px solid var(--line);
  background: #030303;
}
.top-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.brand {
  font-size: 17px;
  font-weight: 700;
  letter-spacing: 0.4px;
}
.row {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.row.tight { gap: 6px; }
.banner {
  margin: 10px 12px 0;
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 8px 10px;
  font-size: 12px;
}
.banner.err { border-color: color-mix(in srgb, var(--err) 70%, var(--line)); color: #ffd1d1; }
.chat-panel {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.messages {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px 14px;
}
.msg {
  max-width: min(760px, 100%);
  border: 1px solid var(--line);
  border-radius: 14px;
  padding: 10px 12px;
  background: #080808;
  align-self: flex-start;
}
.msg.role-user {
  align-self: flex-end;
  border-color: color-mix(in srgb, var(--accent) 52%, var(--line));
  background: #0b120d;
}
.msg.role-assistant {
  border-color: color-mix(in srgb, #3e3e3e 60%, var(--line));
}
.msg.role-tool {
  border-color: color-mix(in srgb, #8c6c2f 58%, var(--line));
}
.msg.role-error {
  border-color: color-mix(in srgb, var(--err) 70%, var(--line));
}
.msg-head {
  font-size: 11px;
  color: var(--muted);
  margin-bottom: 6px;
}
.msg-body {
  font-size: 13px;
  white-space: pre-wrap;
  line-height: 1.42;
}
.msg-meta {
  font-size: 11px;
  color: var(--muted);
  margin-top: 7px;
}
.stream { border-style: dashed; }
.composer {
  border-top: 1px solid var(--line);
  padding: 10px 12px 12px;
  background: #030303;
  display: grid;
  gap: 8px;
}
.status-ribbon {
  border-top: 1px dashed color-mix(in srgb, var(--line) 80%, transparent);
  margin-top: 2px;
  padding-top: 7px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.status-left, .status-right {
  display: flex;
  gap: 6px;
  align-items: center;
  flex-wrap: wrap;
}
.spinner {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 1px solid color-mix(in srgb, var(--line) 75%, transparent);
}
.spinner.run {
  border-color: color-mix(in srgb, var(--accent) 80%, white);
  background: color-mix(in srgb, var(--accent) 80%, transparent);
}
button {
  border: 1px solid var(--line);
  background: #0a0a0a;
  color: var(--text);
  padding: 7px 10px;
  border-radius: 9px;
  cursor: pointer;
  font-family: inherit;
  font-size: 12px;
}
button:hover { border-color: var(--accent); }
button.primary { background: #0f2013; border-color: #1e3a23; }
button.warn { border-color: var(--warn); color: #ffe6bf; }
button.danger { border-color: var(--err); color: #ffd1d1; }
button.on { border-color: var(--ok); color: #c8f7d8; }
button.ghost { background: #050505; }
textarea, input {
  width: 100%;
  border: 1px solid var(--line);
  border-radius: 9px;
  background: #050505;
  color: var(--text);
  padding: 9px 10px;
  font: inherit;
  font-size: 12px;
}
textarea { min-height: 74px; resize: vertical; }
.badge {
  border: 1px solid var(--line);
  border-radius: 999px;
  padding: 2px 8px;
  font-size: 11px;
  color: var(--muted);
}
.badge.ok { border-color: var(--ok); color: #c8f7d8; }
.badge.err { border-color: var(--err); color: #ffd1d1; }
.badge.active { border-color: var(--accent); color: #cbf7da; }
.small { font-size: 11px; color: var(--muted); }
.overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.72);
  backdrop-filter: blur(1.5px);
  z-index: 30;
}
.settings {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(440px, 96vw);
  border-left: 1px solid var(--line);
  background: #020202;
  z-index: 31;
  display: flex;
  flex-direction: column;
}
.settings-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--line);
  padding: 12px;
}
.settings-body {
  padding: 12px;
  overflow: auto;
  display: grid;
  gap: 12px;
}
.section {
  border: 1px solid var(--line);
  border-radius: 10px;
  background: #060606;
  padding: 10px;
  display: grid;
  gap: 8px;
}
.section-title {
  font-size: 11px;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
.kv {
  display: grid;
  grid-template-columns: 1fr;
  gap: 2px;
  font-size: 12px;
}
.kv .k { font-size: 11px; color: var(--muted); }
.loglist {
  max-height: 220px;
  overflow: auto;
  border: 1px solid color-mix(in srgb, var(--line) 80%, transparent);
  border-radius: 8px;
  padding: 6px 8px;
}
.logline {
  font-size: 11px;
  white-space: pre-wrap;
  border-bottom: 1px dashed #252525;
  padding: 4px 0;
}
@media (max-width: 900px) {
  .shell {
    padding: 0;
  }
  .main-col {
    width: 100%;
    border-left: none;
    border-right: none;
  }
  .topbar { padding: 10px; }
  .messages { padding: 12px 10px; }
  .composer { padding: 10px; }
}
"#;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let window = dioxus::desktop::WindowBuilder::new().with_title("jcode");
        let cfg = dioxus::desktop::Config::new()
            .with_window(window)
            .with_menu(None::<dioxus::desktop::muda::Menu>);

        dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(app);
    }

    #[cfg(target_arch = "wasm32")]
    {
        dioxus::LaunchBuilder::web().launch(app);
    }
}

fn app() -> Element {
    let mut model = use_signal(GuiModel::default);
    let mut settings_open = use_signal(|| false);
    let backend = use_hook(backend_impl::BackendBridge::spawn);

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
    let show_settings = *settings_open.read();

    rsx! {
        document::Title { "jcode" }
        style { "{GUI_CSS}" }

        div { class: "shell",
            div { class: "main-col",
                header { class: "topbar",
                    div { class: "top-left",
                        div { class: "brand", "jcode" }
                        div { class: "row tight",
                            if snapshot.connected {
                                span { class: "badge ok", "connected" }
                            } else {
                                span { class: "badge err", "disconnected" }
                            }
                            if snapshot.is_processing {
                                span { class: "badge active", "processing" }
                            } else {
                                span { class: "badge", "idle" }
                            }
                            span { class: "badge", "{snapshot.provider_name}" }
                            span { class: "badge active", "{snapshot.provider_model}" }
                            if snapshot.server_has_update.unwrap_or(false) {
                                span { class: "badge active", "update available" }
                            }
                        }
                    }

                    div { class: "row tight",
                        button {
                            class: "ghost",
                            onclick: move |_| settings_open.set(true),
                            "Settings"
                        }
                    }
                }

                if !snapshot.connected {
                    if let Some(reason) = snapshot.connection_reason.clone() {
                        div { class: "banner err", "{reason}" }
                    }
                }

                main { class: "chat-panel",
                    div { class: "messages",
                        for (idx, message) in snapshot.messages.iter().enumerate() {
                            article {
                                key: "{idx}",
                                class: "msg role-{message.role}",
                                if message.role != "assistant" && message.role != "user" {
                                    div { class: "msg-head", "{message.role}" }
                                }
                                div { class: "msg-body", "{message.content}" }
                                if !message.tool_calls.is_empty() {
                                    div { class: "msg-meta", { format!("tools: {}", message.tool_calls.join(", ")) } }
                                }
                            }
                        }

                        if !snapshot.streaming_text.is_empty() {
                            article {
                                class: "msg role-assistant stream",
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
                            button {
                                class: "ghost",
                                onclick: move |_| settings_open.set(true),
                                "Open settings"
                            }
                        }

                        div { class: "status-ribbon",
                            div { class: "status-left",
                                div { class: if snapshot.is_processing { "spinner run" } else { "spinner" } }
                                if snapshot.is_processing {
                                    span { class: "small", "Thinking / streaming..." }
                                } else {
                                    span { class: "small", "Idle" }
                                }
                                if let Some(session_id) = snapshot.session_id.clone() {
                                    span { class: "badge", "{session_id}" }
                                }
                            }
                            div { class: "status-right",
                                span { class: "badge", "turn in {snapshot.turn_input_tokens} / out {snapshot.turn_output_tokens}" }
                                span { class: "badge", "total in {snapshot.total_input_tokens} / out {snapshot.total_output_tokens}" }
                            }
                        }
                    }
                }
            }

            if show_settings {
                div {
                    class: "overlay",
                    onclick: move |_| settings_open.set(false),
                }
                aside { class: "settings",
                    div { class: "settings-head",
                        div { "Settings" }
                        button {
                            class: "ghost",
                            onclick: move |_| settings_open.set(false),
                            "Close"
                        }
                    }

                    div { class: "settings-body",
                        div { class: "section",
                            div { class: "section-title", "Session" }
                            div { class: "row",
                                if snapshot.connected {
                                    span { class: "badge ok", "connected" }
                                } else {
                                    span { class: "badge err", "disconnected" }
                                }
                                if let Some(session_id) = snapshot.session_id.clone() {
                                    span { class: "badge", "{session_id}" }
                                }
                                if let Some(name) = snapshot.server_name.clone() {
                                    span { class: "badge", "{name}" }
                                }
                                if snapshot.server_has_update.unwrap_or(false) {
                                    span { class: "badge active", "update available" }
                                }
                            }
                            div { class: "kv",
                                div { class: "k", "session tokens" }
                                div { "in {snapshot.total_input_tokens} / out {snapshot.total_output_tokens}" }
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
                        }

                        div { class: "section",
                            div { class: "section-title", "Runtime Features" }
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
                        }

                        div { class: "section",
                            div { class: "section-title", "Model" }
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

                            if !snapshot.available_models.is_empty() {
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

                        div { class: "section",
                            div { class: "section-title", "Session Resume / Interrupt" }
                            div { class: "kv",
                                div { class: "k", "resume session id" }
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

                        div { class: "section",
                            div { class: "section-title", "Context" }
                            if let Some(version) = snapshot.server_version.clone() {
                                div { class: "kv",
                                    div { class: "k", "server version" }
                                    div { "{version}" }
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
                        }

                        div { class: "section",
                            div { class: "section-title", "Activity" }
                            if let Some(error) = snapshot.last_error.clone() {
                                div { class: "small", "last error: {error}" }
                            }
                            div { class: "loglist",
                                for (idx, line) in snapshot.activity_log.iter().enumerate().rev().take(100) {
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
    }
}
