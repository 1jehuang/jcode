//! Opt-in acceptance against real local OpenSSH and a freshly built native CLI.
//! Run: JCODE_SSH_TEST_BINARY=/absolute/path/to/jcode cargo test -p jcode-sdk \
//!   ssh::integration_tests::localhost_native_harness -- --ignored --nocapture
//! No user's SSH config, keys, known_hosts, sessions, or daemon are accessed.

use super::*;
use crate::JcodeClient;
use std::path::Path;
use std::time::Instant;

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(mut command: Command) -> Process {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    Process(command.spawn().unwrap())
}

fn wait_until(mut ready: impl FnMut() -> bool, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn connect(options: &SshConnectOptions, config: &Path) -> Result<JcodeClient> {
    // Keep production argument generation intact, but replace the config only
    // for this fixture. This does not mutate PATH/HOME in the test process.
    let generated = options.command()?;
    let mut command = Command::new("/usr/bin/ssh");
    command.arg("-F").arg(config).args(generated.get_args());
    let transport = SshTransport::spawn_command(command)?;
    JcodeClient::over_ssh(transport, options.clone())
}

#[test]
#[ignore = "requires /usr/bin/sshd, ssh-keygen and JCODE_SSH_TEST_BINARY freshly built CLI"]
fn localhost_native_harness() {
    let binary = std::fs::canonicalize(
        std::env::var_os("JCODE_SSH_TEST_BINARY")
            .expect("set JCODE_SSH_TEST_BINARY to the freshly built jcode CLI"),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let home = root.join("home");
    let runtime = root.join("runtime");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let help = Command::new(&binary)
        .env_clear()
        .env("HOME", &home)
        .env("JCODE_HOME", &home)
        .env("JCODE_NO_TELEMETRY", "1")
        .env("JCODE_RUNTIME_DIR", &runtime)
        .env("XDG_RUNTIME_DIR", &runtime)
        .current_dir(root)
        .args(["--no-update", "api", "--help"])
        .output()
        .unwrap();
    assert!(
        help.status.success() && String::from_utf8_lossy(&help.stdout).contains("--stdio"),
        "JCODE_SSH_TEST_BINARY must be a fresh build supporting api --stdio: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    let daemon_socket = runtime.join("jcode.sock");
    let daemon_log = root.join("daemon.log");
    let mut daemon_command = Command::new(&binary);
    daemon_command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("JCODE_HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("JCODE_RUNTIME_DIR", &runtime)
        .env("JCODE_SOCKET", &daemon_socket)
        .env("JCODE_DEFERRED_AUTH_BOOTSTRAP", "1")
        .env("JCODE_WAKE_MODE", "external")
        .env("JCODE_NO_TELEMETRY", "1")
        .args(["--no-update", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&daemon_log).unwrap());
    let mut daemon = spawn(daemon_command);
    wait_until(
        || {
            if let Some(status) = daemon.0.try_wait().unwrap() {
                panic!(
                    "private daemon exited {status}: {}",
                    std::fs::read_to_string(&daemon_log).unwrap()
                );
            }
            std::os::unix::net::UnixStream::connect(&daemon_socket).is_ok()
        },
        "private native daemon",
    );

    for name in ["host_key", "client_key"] {
        assert!(
            Command::new("/usr/bin/ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(root.join(name))
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::copy(root.join("client_key.pub"), root.join("authorized_keys")).unwrap();
    let user = String::from_utf8(
        Command::new("/usr/bin/id")
            .arg("-un")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let remote_script = root.join("remote.sh");
    std::fs::write(&remote_script, format!(
        "export HOME={} JCODE_HOME={} XDG_RUNTIME_DIR={} JCODE_SOCKET={} JCODE_WAKE_MODE=external JCODE_NO_TELEMETRY=1\nexport JCODE_RUNTIME_DIR=\"$XDG_RUNTIME_DIR\"\ncd \"$HOME\" || exit 1\nexec /bin/sh -c \"$SSH_ORIGINAL_COMMAND\"\n",
        shell_quote(home.to_str().unwrap()), shell_quote(home.to_str().unwrap()),
        shell_quote(runtime.to_str().unwrap()), shell_quote(daemon_socket.to_str().unwrap()),
    )).unwrap();
    let server_config = root.join("sshd_config");
    std::fs::write(&server_config, format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nUseDNS no\nPermitUserRC no\nSetEnv HOME={}\nAllowUsers {}\nForceCommand /bin/sh {}\n",
        root.join("host_key").display(), root.join("sshd.pid").display(),
        root.join("authorized_keys").display(), home.display(), user.trim(), remote_script.display(),
    )).unwrap();
    let sshd_log = root.join("sshd.log");
    let mut sshd_command = Command::new("/usr/bin/sshd");
    sshd_command
        .args(["-D", "-e", "-f"])
        .arg(&server_config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&sshd_log).unwrap());
    let mut sshd = spawn(sshd_command);
    wait_until(
        || {
            if let Some(status) = sshd.0.try_wait().unwrap() {
                panic!(
                    "private sshd exited {status}: {}",
                    std::fs::read_to_string(&sshd_log).unwrap()
                );
            }
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_ok()
        },
        "private sshd",
    );
    let known_hosts = root.join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!(
            "jcode-test {}",
            std::fs::read_to_string(root.join("host_key.pub")).unwrap()
        ),
    )
    .unwrap();
    let client_config = root.join("ssh_config");
    std::fs::write(&client_config, format!(
        "Host jcode-test\n HostName 127.0.0.1\n Port {port}\n User {}\n HostKeyAlias jcode-test\n IdentityFile {}\n IdentitiesOnly yes\n IdentityAgent none\n UserKnownHostsFile {}\n GlobalKnownHostsFile /dev/null\n",
        user.trim(), root.join("client_key").display(), known_hosts.display(),
    )).unwrap();
    let options = SshConnectOptions {
        remote_binary: binary.to_str().unwrap().to_owned(),
        connect_timeout: Duration::from_secs(15),
        client_name: "jcode-sdk-ssh-acceptance".into(),
        ..SshConnectOptions::new("jcode-test")
    };
    let client = connect(&options, &client_config).unwrap_or_else(|error| {
        panic!(
            "{error}; sshd: {}",
            std::fs::read_to_string(&sshd_log).unwrap()
        )
    });
    client.ping().unwrap();
    let session = client
        .create_session(Some(root.to_str().unwrap().to_owned()))
        .unwrap();
    // Native sessions remain provisional until they contain content. Persist
    // one context message without a model turn or any provider credentials.
    let stored = client
        .request(crate::ApiRequest::SendMessage {
            session_id: session.session_id.clone(),
            content: "SSH acceptance context, no model reply requested".into(),
            images: vec![],
            no_reply: true,
        })
        .unwrap();
    assert!(matches!(stored.event, crate::ApiEvent::Ok));
    assert!(
        client
            .list_sessions()
            .unwrap()
            .iter()
            .any(|s| s.session_id == session.session_id)
    );
    drop(client);
    // Closing SSH must not kill the shared native daemon or destroy sessions.
    assert!(daemon.0.try_wait().unwrap().is_none());
    let reconnected = connect(&options, &client_config).unwrap();
    assert_eq!(
        reconnected
            .attach_session(&session.session_id)
            .unwrap()
            .session_id,
        session.session_id
    );
    reconnected.ping().unwrap();
    drop(daemon);
    wait_until(
        || reconnected.is_closed(),
        "stdio exit after daemon shutdown with stdin open",
    );
    assert!(reconnected.ping().is_err());
    drop(reconnected);
    // An unknown key is rejected rather than silently trusted or written.
    std::fs::write(&known_hosts, "").unwrap();
    let error = connect(&options, &client_config)
        .err()
        .expect("unknown host key must fail");
    assert!(
        error.message.contains("Host key verification failed"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(&known_hosts).unwrap(), "");
    eprintln!(
        "real OpenSSH + native api --stdio: handshake, ping, create, list, reconnect/attach, persistent daemon, strict host verification passed"
    );
}
