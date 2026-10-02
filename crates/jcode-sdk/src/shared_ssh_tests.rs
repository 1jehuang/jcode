//! Real OpenSSH multiplexing tests with a tiny deterministic API fixture, no CLI/daemon.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

fn configured(command: Command, config: &std::path::Path) -> Command {
    let mut configured = Command::new("/usr/bin/ssh");
    configured.arg("-F").arg(config).args(command.get_args());
    configured
}

#[test]
fn shared_private_identity_weak_lifetime_and_fail_closed_options() {
    let first = SharedSshTransport::new(SshConnectOptions::new("one")).unwrap();
    let second = SharedSshTransport::new(SshConnectOptions::new("one")).unwrap();
    let path = first.inner.directory.path().to_owned();
    assert_ne!(path, second.inner.directory.path());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let command = first
        .inner
        .options
        .command_with_control(Some((&path.join("master"), false)))
        .unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    for required in [
        "ControlMaster=no",
        "ControlPersist=no",
        "ProxyCommand=/bin/false",
        "ProxyJump=none",
        "StrictHostKeyChecking=yes",
        "ForwardAgent=no",
        "ClearAllForwardings=yes",
    ] {
        assert!(args.contains(&required), "missing {required}");
    }
    let weak = first.downgrade();
    assert!(weak.upgrade().is_some());
    drop(first);
    assert!(weak.upgrade().is_none());
    assert!(!path.exists());
}

#[test]
fn shared_setup_failure_is_cached_and_bounded() {
    let owner = SharedSshTransport::new(SshConnectOptions {
        connect_timeout: Duration::from_millis(80),
        ..SshConnectOptions::new("test")
    })
    .unwrap();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let start = Instant::now();
    let result = owner.connect_with(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        command
    });
    assert_eq!(result.err().unwrap().kind, ErrorKind::StartupTimeout);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(
        owner
            .connect_with(|_| panic!("must not restart failed master"))
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_setup_wait_obeys_callers_deadline_without_spawning() {
    let owner = SharedSshTransport::new(SshConnectOptions {
        connect_timeout: Duration::from_millis(80),
        ..SshConnectOptions::new("test")
    })
    .unwrap();
    let guard = owner.inner.master.lock().unwrap();
    let start = Instant::now();
    std::thread::scope(|scope| {
        let job = scope.spawn(|| owner.connect_with(|_| panic!("setup is still locked")));
        assert_eq!(
            job.join().unwrap().err().unwrap().kind,
            ErrorKind::StartupTimeout
        );
    });
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(guard);
}

#[test]
#[ignore = "requires local /usr/bin/sshd and ssh-keygen; no CLI build or shared daemon"]
fn real_shared_master_concurrent_independent_channels_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for key in ["host", "client"] {
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(root.join(key))
                .status()
                .unwrap()
                .success()
        );
    }
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bridge = root.join("bridge.py");
    let hello = serde_json::to_string(&crate::api::ServerFrame::reply(
        1,
        crate::api::ApiEvent::HelloOk {
            version: crate::api::API_VERSION_MAJOR,
            server: "fixture".into(),
            capabilities: vec![],
        },
    ))
    .unwrap();
    let pong = serde_json::to_string(&crate::api::ServerFrame::reply(
        2,
        crate::api::ApiEvent::Pong,
    ))
    .unwrap();
    std::fs::write(&bridge, format!("import sys,json,os\nhello=json.loads({hello:?})\npong=json.loads({pong:?})\nhello['server']=str(os.getpid())\nfor line in sys.stdin:\n req=json.loads(line)\n reply=hello.copy() if req['id']==1 else pong.copy()\n reply['reply_to']=req['id']\n print(json.dumps(reply),flush=True)\n")).unwrap();
    let server_config = root.join("sshd_config");
    std::fs::write(&server_config, format!("Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nUseDNS no\nMaxSessions 4\nPermitUserRC no\nAllowUsers {}\nForceCommand /usr/bin/python3 {}\n", root.join("host").display(), root.join("pid").display(), root.join("client.pub").display(), user.trim(), bridge.display())).unwrap();
    let mut server_command = Command::new("/usr/bin/sshd");
    server_command.args(["-D", "-e", "-f"]).arg(server_config);
    let server = SshTransport::spawn_command(server_command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err() {
        assert!(Instant::now() < deadline, "{}", server.process.diagnostic());
        std::thread::sleep(Duration::from_millis(10));
    }
    let known = root.join("known");
    std::fs::write(
        &known,
        format!(
            "fixture {}",
            std::fs::read_to_string(root.join("host.pub")).unwrap()
        ),
    )
    .unwrap();
    let config = root.join("config");
    std::fs::write(&config, format!("Host fixture\n HostName 127.0.0.1\n Port {port}\n User {}\n IdentityFile {}\n IdentitiesOnly yes\n IdentityAgent none\n HostKeyAlias fixture\n UserKnownHostsFile {}\n GlobalKnownHostsFile /dev/null\n", user.trim(), root.join("client").display(), known.display())).unwrap();
    let owner = SharedSshTransport::new(SshConnectOptions {
        connect_timeout: Duration::from_secs(5),
        ..SshConnectOptions::new("fixture")
    })
    .unwrap();
    let starts = std::sync::atomic::AtomicUsize::new(0);
    let mut clients = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    owner
                        .connect_with(|command| {
                            if command.get_args().any(|a| a == "ControlMaster=yes") {
                                starts.fetch_add(1, Ordering::SeqCst);
                            }
                            configured(command, &config)
                        })
                        .unwrap()
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let identities: std::collections::HashSet<_> =
        clients.iter().map(|c| c.server.clone()).collect();
    assert_eq!(
        identities.len(),
        clients.len(),
        "every client needs its own remote API process"
    );
    for client in &clients {
        client.ping().unwrap();
    }
    // The server refuses a fifth channel. This must not fall back to a new
    // authentication (which would bypass MaxSessions on a fresh connection).
    let full = Instant::now();
    assert!(owner.connect_with(|cmd| configured(cmd, &config)).is_err());
    assert!(full.elapsed() < Duration::from_secs(2));
    drop(clients.pop());
    let warm = Instant::now();
    let extra = owner.connect_with(|cmd| configured(cmd, &config)).unwrap();
    eprintln!(
        "warm real-SSH independent API channel: {:?}",
        warm.elapsed()
    );
    extra.ping().unwrap();
    drop(extra);
    // A live master closes on the final client drop even if an EventStream
    // keeps its channel's Inner alive. Other transport instances are unaffected.
    let independent = SharedSshTransport::new(owner.inner.options.clone()).unwrap();
    let independent_client = independent
        .connect_with(|cmd| configured(cmd, &config))
        .unwrap();
    let independent_stream = independent_client.events(None);
    let independent_weak = independent.downgrade();
    let independent_path = independent.inner.directory.path().to_owned();
    let independent_pid = independent
        .inner
        .master
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .child
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .id();
    drop(independent);
    assert!(independent_weak.upgrade().is_some());
    drop(independent_client);
    assert!(independent_weak.upgrade().is_none());
    assert!(!independent_path.exists());
    assert_eq!(
        unsafe { libc::waitpid(independent_pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    drop(independent_stream);
    clients[0].ping().unwrap();
    let weak = owner.downgrade();
    let path = owner.inner.directory.path().to_owned();
    let process = owner
        .inner
        .master
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let pid = process.child.lock().unwrap().as_ref().unwrap().id();
    drop(process);
    drop(owner);
    assert!(weak.upgrade().is_some());
    assert!(path.exists());
    clients[0].ping().unwrap();
    let retained_stream = clients[0].events(None);
    // A dead master must not silently reconnect, even though sshd remains live.
    let retained = weak.upgrade().unwrap();
    retained
        .inner
        .master
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .shutdown();
    let failed = Instant::now();
    assert!(
        retained
            .connect_with(|cmd| configured(cmd, &config))
            .is_err()
    );
    assert!(failed.elapsed() < Duration::from_secs(2));
    drop(retained);
    drop(clients);
    let deadline = Instant::now() + Duration::from_secs(2);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(weak.upgrade().is_none());
    assert!(!path.exists());
    drop(retained_stream);
    assert_eq!(
        unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
