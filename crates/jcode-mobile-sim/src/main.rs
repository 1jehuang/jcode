use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use jcode_mobile_core::{ScenarioName, SimulatorAction};
use jcode_mobile_sim::{
    AutomationRequest, default_socket_path, request_status, run_server, send_request,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "jcode-mobile-sim")]
#[command(about = "Headless-first mobile simulator and automation CLI for jcode")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "onboarding")]
        scenario: String,
    },
    Start {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "onboarding")]
        scenario: String,
    },
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    State {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    Tree {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    Log {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Reset {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    LoadScenario {
        #[arg(long)]
        socket: Option<PathBuf>,
        scenario: String,
    },
    SetField {
        #[arg(long)]
        socket: Option<PathBuf>,
        field: String,
        value: String,
    },
    Tap {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
    },
    DispatchJson {
        #[arg(long)]
        socket: Option<PathBuf>,
        action_json: String,
    },
    Shutdown {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { socket, scenario } => {
            let socket = socket.unwrap_or_else(default_socket_path);
            let scenario = parse_scenario(&scenario)?;
            run_server(&socket, scenario).await
        }
        Command::Start { socket, scenario } => start_background(socket, &scenario).await,
        Command::Status { socket } => {
            let status = request_status(&resolve_socket(socket)).await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::State { socket } => {
            print_result(send_simple(&resolve_socket(socket), "state", Value::Null).await?)
        }
        Command::Tree { socket } => {
            print_result(send_simple(&resolve_socket(socket), "tree", Value::Null).await?)
        }
        Command::Log { socket, limit } => print_result(
            send_simple(&resolve_socket(socket), "log", json!({ "limit": limit })).await?,
        ),
        Command::Reset { socket } => {
            print_result(send_simple(&resolve_socket(socket), "reset", Value::Null).await?)
        }
        Command::LoadScenario { socket, scenario } => print_result(
            send_simple(
                &resolve_socket(socket),
                "load_scenario",
                json!({ "scenario": parse_scenario(&scenario)?.as_str() }),
            )
            .await?,
        ),
        Command::SetField {
            socket,
            field,
            value,
        } => {
            let action = map_set_field(&field, value)?;
            print_result(dispatch_action(&resolve_socket(socket), action).await?)
        }
        Command::Tap { socket, node_id } => print_result(
            dispatch_action(
                &resolve_socket(socket),
                SimulatorAction::TapNode { node_id },
            )
            .await?,
        ),
        Command::DispatchJson {
            socket,
            action_json,
        } => {
            let action: SimulatorAction =
                serde_json::from_str(&action_json).with_context(|| "parse action JSON")?;
            print_result(dispatch_action(&resolve_socket(socket), action).await?)
        }
        Command::Shutdown { socket } => {
            print_result(send_simple(&resolve_socket(socket), "shutdown", Value::Null).await?)
        }
    }
}

fn resolve_socket(socket: Option<PathBuf>) -> PathBuf {
    socket.unwrap_or_else(default_socket_path)
}

fn parse_scenario(input: &str) -> Result<ScenarioName> {
    ScenarioName::parse(input).ok_or_else(|| anyhow!("unknown scenario: {input}"))
}

fn map_set_field(field: &str, value: String) -> Result<SimulatorAction> {
    match field {
        "host" | "pair.host" => Ok(SimulatorAction::SetHost { value }),
        "port" | "pair.port" => Ok(SimulatorAction::SetPort { value }),
        "pair_code" | "code" | "pair.code" => Ok(SimulatorAction::SetPairCode { value }),
        "device_name" | "pair.device_name" => Ok(SimulatorAction::SetDeviceName { value }),
        "draft" | "chat.draft" => Ok(SimulatorAction::SetDraft { value }),
        _ => bail!("unknown field: {field}"),
    }
}

async fn dispatch_action(socket: &Path, action: SimulatorAction) -> Result<Value> {
    send_simple(socket, "dispatch", json!({ "action": action })).await
}

async fn send_simple(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let response = send_request(
        socket,
        AutomationRequest {
            id: format!("{}-{}", method, unique_id()),
            method: method.to_string(),
            params,
        },
    )
    .await?;

    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| format!("{} failed", method))
        );
    }
    Ok(response.result)
}

fn print_result(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn start_background(socket: Option<PathBuf>, scenario: &str) -> Result<()> {
    let socket = resolve_socket(socket);
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .arg("--scenario")
        .arg(scenario);
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    command.stdin(std::process::Stdio::null());
    let _child = command
        .spawn()
        .with_context(|| "spawn background simulator")?;

    for _ in 0..100 {
        if socket.exists() && request_status(&socket).await.is_ok() {
            println!("{}", socket.display());
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    bail!("simulator did not become ready at {}", socket.display())
}

fn unique_id() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
