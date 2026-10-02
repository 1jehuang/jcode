#!/usr/bin/env python3
"""Headless, no-inference acceptance through the actual Rust SDK and private daemons.

Build first:
  scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode
  scripts/dev_cargo.sh build --profile selfdev -p jcode-harness-api-server --bin jcode-harness-api-bridge
Run:
  python3 scripts/test_desktop_selfdev.py --desktop-repo ../jcode-desktop

No shared sockets, credentials, Desktop host, compositor, or live sessions are used.
The SDK harness is generated outside the repository for later review. Artifacts
are retained, including logs and the exact Rust source. Context preparation
freezes the real provider tool snapshot but never invokes provider inference.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time


SDK_SOURCE = r'''
use jcode_sdk::{api::ApiRequest, ConnectOptions, JcodeClient};
use serde_json::{json, Value};
use std::{io::{BufRead, BufReader, Write}, os::unix::net::UnixStream, path::PathBuf, time::Duration};

fn debug(socket: &str, session: &str, command: &str, ok: bool) -> Value {
    let mut stream = UnixStream::connect(socket).expect("private debug connect");
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    writeln!(stream, "{}", json!({"type":"debug_command", "id":1,
        "session_id":session, "command":command})).unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0, "debug socket closed");
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["id"] != 1 { continue; }
        assert_eq!(frame["ok"], ok, "{command}: {frame}");
        if !ok { return frame; }
        return serde_json::from_str(frame["output"].as_str().unwrap()).unwrap();
    }
}
fn connect(api: &str) -> JcodeClient {
    JcodeClient::connect(ConnectOptions {
        socket_path: Some(PathBuf::from(api)), ensure_runtime: false,
        client_name: "desktop-selfdev-real-sdk-acceptance".into(),
        request_timeout: Some(Duration::from_secs(20)),
    }).expect("real SDK handshake")
}
fn names(context: &Value) -> Vec<&str> {
    context["prepared_tools"].as_array().unwrap().iter()
        .map(|tool| tool["name"].as_str().unwrap()).collect()
}
fn verify(api: &str, dbg: &str, cwd: &str, mode: &str, desktop_root: &str) {
    let client = connect(api);
    let session = client.create_session(Some(cwd.into())).expect("SDK create");
    let id = &session.session_id;
    let before = debug(dbg, id, "agent:info", true);
    let context = debug(dbg, id, "agent:context:prepare", true);
    assert_eq!(context["mode"], mode, "{context}");
    assert_eq!(context["tools_locked"], true);
    assert_eq!(context["prepared_tools"], context["effective_tools"]);
    let tool_names = names(&context);
    assert_eq!(context["locked_tool_names"], json!(tool_names));
    let prompt = context["system_prompt"]["static"].as_str().unwrap();
    assert_eq!(prompt.contains("# Jcode Desktop Self-Development Mode"), mode == "desktop");
    assert_eq!(prompt.contains("# Self-Development Mode"), mode == "cli");
    match mode {
        "desktop" => {
            assert_eq!(context["is_canary"], false);
            assert!(tool_names.contains(&"desktop_selfdev"));
            for tool in ["selfdev", "debug_socket", "jcode_docs"] {
                assert!(!tool_names.contains(&tool));
            }
            let definition = context["prepared_tools"].as_array().unwrap().iter()
                .find(|t| t["name"] == "desktop_selfdev").unwrap();
            assert!(definition["input_schema"]["properties"]["action"]["enum"]
                .as_array().unwrap().contains(&json!("status")));
            let status = debug(dbg, id, r#"tool:desktop_selfdev {"action":"status"}"#, true);
            let status: Value = serde_json::from_str(status["output"].as_str().unwrap()).unwrap();
            assert_eq!(status["mode"], "desktop");
            assert_eq!(status["repo"], desktop_root);
            assert!(status["instance"].is_null(), "must not see a live Desktop host");
            let test = debug(dbg, id,
                r#"tool:desktop_selfdev {"action":"test","command":"printf 'desktop-sdk-no-inference\\n'; pwd","timeout_seconds":10}"#, true);
            let output = test["output"].as_str().unwrap();
            assert!(output.contains("desktop-sdk-no-inference"), "{output}");
            assert!(output.contains(desktop_root), "{output}");
            for tool in ["selfdev", "debug_socket"] {
                debug(dbg, id, &format!("tool:{tool} {{\"action\":\"status\"}}"), false);
            }
        }
        "cli" => {
            assert_eq!(context["is_canary"], true);
            assert!(tool_names.contains(&"selfdev"));
            assert!(!tool_names.contains(&"desktop_selfdev"));
            debug(dbg, id, r#"tool:desktop_selfdev {"action":"status"}"#, false);
        }
        "regular" => {
            assert_eq!(context["is_canary"], false);
            assert!(!tool_names.contains(&"selfdev"));
            assert!(!tool_names.contains(&"desktop_selfdev"));
            debug(dbg, id, r#"tool:desktop_selfdev {"action":"status"}"#, false);
        }
        _ => panic!("unknown mode"),
    }
    let after = debug(dbg, id, "agent:info", true);
    assert_eq!(before["session"]["message_count"], after["session"]["message_count"]);
    assert_eq!(before["token_usage"], after["token_usage"]);
    assert_eq!(after["token_usage"]["output"], 0);
    // Persist a marker using the SDK's explicit no_reply path, then reattach.
    let reply = client.request(ApiRequest::SendMessage {
        session_id: id.clone(), content: "desktop SDK acceptance context only".into(),
        images: vec![], system_reminder: None, no_reply: true,
    }).expect("SDK context-only persistence");
    assert!(matches!(reply.event, jcode_sdk::api::ApiEvent::Ok));
    let history = client.get_history(id).expect("SDK history");
    let observer = connect(api);
    let attached = observer.attach_session(id).expect("SDK reattach");
    assert_eq!(&attached.session_id, id);
    assert_eq!(attached.working_dir.as_deref(), Some(cwd));
    assert_eq!(serde_json::to_value(observer.get_history(id).unwrap()).unwrap(),
        serde_json::to_value(history).unwrap());
    let restored = debug(dbg, id, "agent:context:prepare", true);
    assert_eq!(restored["mode"], mode);
    assert_eq!(restored["prepared_tools"], context["prepared_tools"]);
    assert_eq!(debug(dbg, id, "agent:info", true)["token_usage"]["output"], 0);
    println!("{}", json!({"mode":mode,"cwd":cwd,"session_id":id,
        "sdk_create":true,"sdk_reattach":true,"provider_prompt_and_tools":true,
        "actual_desktop_status_and_test":mode=="desktop","model_calls":0}));
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    assert_eq!(a.len(), 8);
    verify(&a[1], &a[2], &a[3], "desktop", &a[3]);
    verify(&a[1], &a[2], &a[4], "desktop", &a[3]);
    verify(&a[1], &a[2], &a[5], "desktop", &a[3]);
    verify(&a[1], &a[2], &a[6], "regular", &a[3]);
    verify(&a[1], &a[2], &a[7], "cli", &a[3]);
}
'''


def wait_socket(path, process, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Private process exited {process.returncode}. See retained logs.")
        try:
            with socket.socket(socket.AF_UNIX) as stream:
                stream.settimeout(.2)
                stream.connect(str(path))
            return
        except OSError:
            time.sleep(.05)
    raise TimeoutError(f"Private socket did not become ready: {path}")


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jcode-binary', type=Path, default=repo / 'target/selfdev/jcode')
    parser.add_argument('--bridge-binary', type=Path, default=repo / 'target/selfdev/jcode-harness-api-bridge')
    parser.add_argument('--desktop-repo', type=Path, default=repo.parent / 'jcode-desktop')
    parser.add_argument('--output-dir', type=Path, help='New artifact directory outside both repositories')
    parser.add_argument('--compile-only', action='store_true', help='Compile/review the real SDK harness without starting any runtime')
    args = parser.parse_args()
    binary = args.jcode_binary.resolve(strict=True)
    bridge = args.bridge_binary.resolve(strict=True)
    desktop = args.desktop_repo.resolve(strict=True)
    nested = desktop / 'crates/jcode-desktop-ui/src'
    assert nested.is_dir(), 'A real Desktop source checkout is required'
    if args.output_dir:
        root = args.output_dir.resolve()
        for checkout in (repo, desktop):
            if root == checkout or checkout in root.parents:
                parser.error('--output-dir must be outside both repositories')
        root.mkdir(mode=0o700, parents=True, exist_ok=False)
    else:
        scratch = Path(os.environ.get('JCODE_SCRATCH_DIR', str(Path.home() / '.cache/jcode-acceptance')))
        scratch = scratch.resolve()
        for checkout in (repo, desktop):
            if scratch == checkout or checkout in scratch.parents:
                parser.error('JCODE_SCRATCH_DIR must be outside both repositories')
        scratch.mkdir(mode=0o700, parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(prefix='desktop-selfdev-', dir=scratch))
    print(f'Artifacts: {root}', flush=True)
    if len(os.fsencode(str(root / 'runtime/daemon-debug.sock'))) >= 104:
        parser.error('Artifact path is too long for private Unix sockets. Choose a shorter --output-dir.')
    for directory in ['home', 'runtime', 'config', 'cache', 'data', 'state', 'jcode', 'tmp', 'sdk/src', 'regular/jcode-desktop']:
        (root / directory).mkdir(mode=0o700, parents=True, exist_ok=True)
    (root / 'desktop-link').symlink_to(nested, target_is_directory=True)
    (root / 'jcode/config.toml').write_text('[features]\nmemory = false\n[telemetry]\nenabled = false\n')
    (root / 'sdk/Cargo.toml').write_text(
        '[package]\nname = "desktop-selfdev-sdk-acceptance"\nversion = "0.0.0"\nedition = "2024"\n'
        '[workspace]\n[dependencies]\nserde_json = "1"\njcode-sdk = { path = '
        + json.dumps(str(repo / 'crates/jcode-sdk')) + ' }\n')
    (root / 'sdk/src/main.rs').write_text(SDK_SOURCE)
    cargo = shutil.which('cargo')
    if not cargo:
        raise RuntimeError('cargo is required to compile the real Rust SDK client')
    with (root / 'sdk-build.log').open('w') as log:
        subprocess.run([cargo, 'build', '--offline', '--manifest-path', str(root / 'sdk/Cargo.toml'),
                        '--target-dir', str(root / 'sdk-target')], cwd=repo,
                       stdout=log, stderr=subprocess.STDOUT, check=True, timeout=600)
    if args.compile_only:
        print('PASS: real Rust SDK acceptance client compiled. No runtime was started.')
        return
    env = {
        'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C.UTF-8',
        'HOME': str(root / 'home'), 'XDG_RUNTIME_DIR': str(root / 'runtime'),
        'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_CACHE_HOME': str(root / 'cache'),
        'XDG_DATA_HOME': str(root / 'data'), 'XDG_STATE_HOME': str(root / 'state'),
        'TMPDIR': str(root / 'tmp'), 'JCODE_HOME': str(root / 'jcode'),
        'JCODE_RUNTIME_DIR': str(root / 'runtime'), 'JCODE_SOCKET': str(root / 'runtime/daemon.sock'),
        'JCODE_API_SOCKET': str(root / 'runtime/api.sock'), 'JCODE_DEBUG_CONTROL': '1',
        'JCODE_NO_TELEMETRY': '1', 'JCODE_TEMP_SERVER': '1',
        'JCODE_SERVER_OWNER_PID': str(os.getpid()), 'JCODE_TEMP_SERVER_IDLE_SECS': '300',
    }
    processes, logs = [], []
    def launch(name, command):
        log = (root / f'{name}.log').open('w')
        logs.append(log)
        process = subprocess.Popen([str(arg) for arg in command], cwd=root, env=env,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        processes.append(process)
        return process
    try:
        daemon = launch('daemon', [binary, '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait_socket(Path(env['JCODE_SOCKET']), daemon)
        debug = root / 'runtime/daemon-debug.sock'
        wait_socket(debug, daemon)
        adapter = launch('bridge', [bridge, env['JCODE_API_SOCKET'], env['JCODE_SOCKET']])
        wait_socket(Path(env['JCODE_API_SOCKET']), adapter)
        helper = root / 'sdk-target/debug/desktop-selfdev-sdk-acceptance'
        result = subprocess.run([str(helper), env['JCODE_API_SOCKET'], str(debug), str(desktop),
                                 str(nested), str(root / 'desktop-link'),
                                 str(root / 'regular/jcode-desktop'), str(repo)],
                                cwd=root, env=env, text=True, capture_output=True, timeout=180)
        (root / 'acceptance.stdout').write_text(result.stdout)
        (root / 'acceptance.stderr').write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f'SDK acceptance failed ({result.returncode}): {result.stderr[-5000:]}')
        print(result.stdout, end='')
        evidence = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
        assert len(evidence) == 5, evidence
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print('PASS: real SDK create/reattach, prepared provider context, actual Desktop status/test, and mode denials. No inference.')
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
