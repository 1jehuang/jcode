//! In-process SSH transport (feature `ssh-native`).
//!
//! Pure Rust (russh), so it works where no `ssh` binary exists, such as iOS.
//! It is deliberately narrow: public-key auth with a key the caller holds in
//! memory, and the server's host key must match one of the pinned keys. No
//! agent, no config files, no known_hosts lookups, no password prompts.
//! The remote runs `jcode api --stdio`, exactly like [`crate::SshConnectOptions`].

use crate::cloud::CloudTarget;
use crate::{ConnectOptions, Error, ErrorKind, JcodeClient, Result, Transport};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// An in-memory SSH private key.
#[derive(Clone)]
pub struct NativeSshKey {
    private: Arc<russh::keys::PrivateKey>,
    public_openssh: String,
}

impl std::fmt::Debug for NativeSshKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSshKey")
            .field("public_openssh", &self.public_openssh)
            .finish_non_exhaustive()
    }
}

impl NativeSshKey {
    /// A fresh ed25519 key. `comment` ends up in the OpenSSH public key line.
    pub fn generate(comment: &str) -> Result<Self> {
        let private =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .map_err(|e| {
                    Error::new(
                        ErrorKind::ConnectFailed,
                        format!("Could not generate a connection key: {e}"),
                    )
                })?;
        Self::from_private(private, comment)
    }

    /// Load an OpenSSH private key (unencrypted).
    pub fn from_openssh(pem: &str) -> Result<Self> {
        let private = russh::keys::PrivateKey::from_openssh(pem).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOption,
                format!("invalid SSH private key: {e}"),
            )
        })?;
        let comment = private.comment().to_string();
        Self::from_private(private, &comment)
    }

    fn from_private(private: russh::keys::PrivateKey, comment: &str) -> Result<Self> {
        let mut public_openssh = private
            .public_key()
            .to_openssh()
            .map_err(|e| Error::new(ErrorKind::InvalidOption, e.to_string()))?;
        let comment: String = comment
            .chars()
            .filter(|c| c.is_ascii_graphic())
            .take(64)
            .collect();
        if !comment.is_empty() {
            public_openssh.push(' ');
            public_openssh.push_str(&comment);
        }
        Ok(Self {
            private: Arc::new(private),
            public_openssh,
        })
    }

    /// `ssh-ed25519 AAAA... comment`, for authorized_keys or Jcode Cloud.
    pub fn public_openssh(&self) -> &str {
        &self.public_openssh
    }
}

/// How [`JcodeClient::connect_native_ssh`] reaches a host.
#[derive(Debug, Clone)]
pub struct NativeSshOptions {
    pub address: std::net::IpAddr,
    pub port: u16,
    pub user: String,
    /// OpenSSH public key lines (`type base64 [comment]`). The server must
    /// present one of these. Empty is rejected: there is no TOFU.
    pub host_keys: Vec<String>,
    pub key: NativeSshKey,
    /// Remote executable name or literal path.
    pub remote_binary: String,
    /// TCP connect, SSH handshake, auth, exec and API hello, in total.
    pub connect_timeout: Duration,
    pub request_timeout: Option<Duration>,
    pub client_name: String,
    pub keepalive_interval: Option<Duration>,
}

impl NativeSshOptions {
    pub fn new(target: CloudTarget, key: NativeSshKey) -> Self {
        let defaults = ConnectOptions::default();
        Self {
            address: target.address,
            port: target.port,
            user: target.user,
            host_keys: target.host_keys,
            key,
            remote_binary: "jcode".into(),
            connect_timeout: Duration::from_secs(45),
            request_timeout: defaults.request_timeout,
            client_name: defaults.client_name,
            keepalive_interval: Some(Duration::from_secs(20)),
        }
    }

    fn remote_command(&self) -> Result<String> {
        if self.remote_binary.is_empty()
            || self.remote_binary.starts_with('-')
            || self.remote_binary.chars().any(char::is_control)
        {
            return Err(Error::new(
                ErrorKind::InvalidOption,
                "invalid remote_binary",
            ));
        }
        Ok(format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {} --no-update api --stdio",
            crate::ssh::shell_quote(&self.remote_binary)
        ))
    }
}

struct PinnedHost {
    keys: Vec<russh::keys::PublicKey>,
    mismatch: Arc<AtomicBool>,
}

impl russh::client::Handler for PinnedHost {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        // Only raw pinned keys are trusted. Certificates must not silently
        // become a new trust path when upgrading russh's handler API.
        let ok = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                self.keys.iter().any(|k| k.key_data() == key.key_data())
            }
            russh::keys::PublicKeyOrCertificate::Certificate(_) => false,
        };
        if !ok {
            self.mismatch.store(true, Ordering::Relaxed);
        }
        Ok(ok)
    }
}

enum Outgoing {
    Data(Vec<u8>),
    Close,
}

/// A connected `jcode api --stdio` channel over in-process SSH.
pub struct NativeSshTransport {
    incoming: mpsc::Receiver<Vec<u8>>,
    outgoing: tokio::sync::mpsc::UnboundedSender<Outgoing>,
}

impl NativeSshTransport {
    /// Connect, authenticate and start the remote API bridge. Blocks.
    pub fn connect(options: &NativeSshOptions) -> Result<Self> {
        let command = options.remote_command()?;
        let keys: Vec<_> = options
            .host_keys
            .iter()
            .map(|key| {
                russh::keys::PublicKey::from_openssh(key).map_err(|error| {
                    Error::new(
                        ErrorKind::InvalidOption,
                        format!("invalid pinned host key: {error}"),
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if keys.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOption,
                "no valid pinned host keys",
            ));
        }
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();
        let options = options.clone();
        std::thread::Builder::new()
            .name("jcode-sdk-native-ssh".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        // A dropped receiver means the caller already abandoned startup.
                        if ready_tx.send(Err(Error::new(ErrorKind::ConnectFailed, e.to_string()))).is_err() {
                            return;
                        }
                        return;
                    }
                };
                rt.block_on(async move {
                    let mismatch = Arc::new(AtomicBool::new(false));
                    let opened = tokio::time::timeout(
                        options.connect_timeout,
                        open(&options, keys, Arc::clone(&mismatch), &command),
                    )
                    .await;
                    let (session, mut channel) = match opened {
                        Ok(Ok(pair)) => pair,
                        Ok(Err(e)) => {
                            let e = if mismatch.load(Ordering::Relaxed) {
                                Error::new(ErrorKind::HostKeyMismatch, "the host's SSH key does not match the pinned key")
                            } else {
                                e
                            };
                            if ready_tx.send(Err(e)).is_err() {
                                return;
                            }
                            return;
                        }
                        Err(_) => {
                            if ready_tx.send(Err(Error::new(ErrorKind::StartupTimeout, "SSH connection timed out"))).is_err() {
                                return;
                            }
                            return;
                        }
                    };
                    let caller_ready = ready_tx.send(Ok(())).is_ok();
                    loop {
                        if !caller_ready {
                            break;
                        }
                        tokio::select! {
                            msg = channel.wait() => match msg {
                                Some(russh::ChannelMsg::Data { data }) => {
                                    if in_tx.send(data.to_vec()).is_err() { break; }
                                }
                                Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => break,
                                Some(_) => {}
                            },
                            out = out_rx.recv() => match out {
                                Some(Outgoing::Data(bytes)) => {
                                    if channel.data(&bytes[..]).await.is_err() { break; }
                                }
                                Some(Outgoing::Close) | None => {
                                    // Shutdown is best effort: timeout or EOF failure both close the channel.
                                    match tokio::time::timeout(Duration::from_secs(2), channel.eof()).await {
                                        Ok(Ok(())) | Ok(Err(_)) | Err(_) => {}
                                    }
                                    break;
                                }
                            },
                        }
                    }
                    if session.disconnect(russh::Disconnect::ByApplication, "", "en").await.is_err() {
                        // The peer may already have closed. Dropping the handle still tears down SSH.
                        drop(session);
                    }
                    // Dropping in_tx signals EOF to the reader.
                });
            })
            .map_err(|e| Error::new(ErrorKind::ConnectFailed, e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                incoming: in_rx,
                outgoing: out_tx,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::new(
                ErrorKind::ConnectFailed,
                "SSH thread exited during startup",
            )),
        }
    }
}

async fn open(
    options: &NativeSshOptions,
    keys: Vec<russh::keys::PublicKey>,
    mismatch: Arc<AtomicBool>,
    command: &str,
) -> Result<(
    russh::client::Handle<PinnedHost>,
    russh::Channel<russh::client::Msg>,
)> {
    let failed = |m: String| Error::new(ErrorKind::ConnectFailed, m);
    let config = Arc::new(russh::client::Config {
        keepalive_interval: options.keepalive_interval,
        ..Default::default()
    });
    let mut session = russh::client::connect(
        config,
        (options.address, options.port),
        PinnedHost { keys, mismatch },
    )
    .await
    .map_err(|e| {
        failed(format!(
            "Could not connect to {}:{}: {e}",
            options.address, options.port
        ))
    })?;
    let auth = session
        .authenticate_publickey(
            &options.user,
            russh::keys::PrivateKeyWithHashAlg::new(Arc::clone(&options.key.private), None),
        )
        .await
        .map_err(|e| failed(e.to_string()))?;
    if !auth.success() {
        return Err(failed("The host rejected the SSH key".into()));
    }
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| failed(e.to_string()))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| failed(e.to_string()))?;
    Ok((session, channel))
}

struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct ChannelWriter(tokio::sync::mpsc::UnboundedSender<Outgoing>);

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.send(Outgoing::Data(data.to_vec())).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "SSH channel closed")
        })?;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Transport for NativeSshTransport {
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let tx = self.outgoing.clone();
        Some(Arc::new(move || {
            match tx.send(Outgoing::Close) {
                Ok(()) => {}
                // The worker has already stopped, which satisfies shutdown.
                Err(_) => {}
            }
        }))
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let reader = ChannelReader {
            rx: self.incoming,
            buf: Vec::new(),
            pos: 0,
        };
        Ok((
            Box::new(BufReader::new(reader)),
            Box::new(ChannelWriter(self.outgoing)),
        ))
    }
}

impl JcodeClient {
    /// Connect to `jcode api --stdio` on a host over in-process SSH, with the
    /// host key pinned. No `ssh` binary, config, or agent is used. Dropping the
    /// last client clone closes the SSH connection, not the remote daemon.
    pub fn connect_native_ssh(options: NativeSshOptions) -> Result<Self> {
        let transport = NativeSshTransport::connect(&options)?;
        Self::connect_with(
            Box::new(transport),
            ConnectOptions {
                socket_path: Some(std::path::PathBuf::new()),
                client_name: options.client_name.clone(),
                request_timeout: options.request_timeout.or(Some(options.connect_timeout)),
                ensure_runtime: false,
                supports_user_response: false,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSshServer {
        authorized: russh::keys::PublicKey,
    }

    impl russh::server::Handler for TestSshServer {
        type Error = russh::Error;

        async fn auth_publickey(
            &mut self,
            user: &str,
            key: russh::server::VerifiedPublicKey<'_>,
        ) -> std::result::Result<russh::server::Auth, Self::Error> {
            Ok(
                if user == "test" && key.public_key.key_data() == self.authorized.key_data() {
                    russh::server::Auth::Accept
                } else {
                    russh::server::Auth::reject()
                },
            )
        }

        async fn channel_open_session(
            &mut self,
            _channel: russh::Channel<russh::server::Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut russh::server::Session,
        ) -> std::result::Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            command: &[u8],
            session: &mut russh::server::Session,
        ) -> std::result::Result<(), Self::Error> {
            assert!(String::from_utf8_lossy(command).ends_with("api --stdio"));
            session.channel_success(channel)?;
            session.data(channel, b"encrypted SSH channel ready\n".to_vec())?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn ed25519_handshake_auth_exec_and_wrong_pin() {
        let host = NativeSshKey::generate("host").unwrap();
        let client = NativeSshKey::generate("client").unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![(*host.private).clone()],
            ..Default::default()
        });
        for correct_pin in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let config = Arc::clone(&config);
            let authorized = client.private.public_key().clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                if let Ok(session) =
                    russh::server::run_stream(config, stream, TestSshServer { authorized }).await
                {
                    let _ = session.await;
                }
            });
            let options = NativeSshOptions::new(
                CloudTarget {
                    address: addr.ip(),
                    port: addr.port(),
                    user: "test".into(),
                    host_keys: vec![host.public_openssh().into()],
                },
                client.clone(),
            );
            let pinned = if correct_pin { &host } else { &client };
            let mismatch = Arc::new(AtomicBool::new(false));
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                open(
                    &options,
                    vec![pinned.private.public_key().clone()],
                    Arc::clone(&mismatch),
                    &options.remote_command().unwrap(),
                ),
            )
            .await
            .unwrap();
            if correct_pin {
                let (session, mut channel) = result.unwrap();
                let data = tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        if let Some(russh::ChannelMsg::Data { data }) = channel.wait().await {
                            break data;
                        }
                    }
                })
                .await
                .unwrap();
                assert_eq!(&data[..], b"encrypted SSH channel ready\n");
                assert!(!mismatch.load(Ordering::Relaxed));
                session
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await
                    .unwrap();
            } else {
                assert!(result.is_err());
                assert!(mismatch.load(Ordering::Relaxed));
            }
            server.abort();
        }
    }

    #[test]
    fn keys_are_fresh_openssh_ed25519_with_safe_comment() {
        let a = NativeSshKey::generate("jcode ios\n").unwrap();
        let b = NativeSshKey::generate("x").unwrap();
        assert!(a.public_openssh().starts_with("ssh-ed25519 "));
        assert!(a.public_openssh().ends_with(" jcodeios"));
        assert_ne!(a.public_openssh(), b.public_openssh());
    }

    #[test]
    fn rejects_unpinned_and_unsafe_options() {
        let key = NativeSshKey::generate("t").unwrap();
        let target = CloudTarget {
            address: "127.0.0.1".parse().unwrap(),
            port: 1,
            user: "u".into(),
            host_keys: vec!["ssh-ed25519 notbase64!".into()],
        };
        let err = NativeSshTransport::connect(&NativeSshOptions::new(target.clone(), key.clone()))
            .err()
            .unwrap();
        assert_eq!(err.kind, ErrorKind::InvalidOption);
        let mut opts = NativeSshOptions::new(target, key);
        opts.remote_binary = "-oProxyCommand=x".into();
        assert!(opts.remote_command().is_err());
    }

    #[tokio::test]
    async fn pinned_subject_does_not_implicitly_trust_host_certificates() {
        use russh::client::Handler;
        use russh::keys::ssh_key::certificate::{Builder, CertType};
        let host = NativeSshKey::generate("host").unwrap();
        let ca = NativeSshKey::generate("untrusted-ca").unwrap();
        let mut builder = Builder::new_with_random_nonce(
            &mut rand::rng(),
            host.private.public_key().clone(),
            0,
            u64::MAX,
        )
        .unwrap();
        builder.cert_type(CertType::Host).unwrap();
        builder.valid_principal("localhost").unwrap();
        let cert = builder.sign(&*ca.private).unwrap();
        let mismatch = Arc::new(AtomicBool::new(false));
        let mut handler = PinnedHost {
            keys: vec![host.private.public_key().clone()],
            mismatch: Arc::clone(&mismatch),
        };
        assert!(!handler.check_server_key(&cert.into()).await.unwrap());
        assert!(mismatch.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn host_key_mismatch_is_recorded_without_poisonable_state() {
        use russh::client::Handler;
        let mismatch = Arc::new(AtomicBool::new(false));
        let mut handler = PinnedHost {
            keys: Vec::new(),
            mismatch: Arc::clone(&mismatch),
        };
        let key = NativeSshKey::generate("host").unwrap();
        assert!(
            !handler
                .check_server_key(&key.private.public_key().clone().into())
                .await
                .unwrap()
        );
        assert!(mismatch.load(Ordering::Relaxed));
    }

    /// The reader/writer bridge speaks the SDK protocol end to end.
    #[test]
    fn channel_bridge_carries_frames_to_the_client() {
        use crate::api::{API_VERSION_MAJOR, ApiEvent, ServerFrame};
        let (in_tx, in_rx) = mpsc::channel();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let transport = NativeSshTransport {
            incoming: in_rx,
            outgoing: out_tx,
        };
        std::thread::spawn(move || {
            let Some(Outgoing::Data(hello)) = out_rx.blocking_recv() else {
                panic!()
            };
            let hello: serde_json::Value = serde_json::from_slice(&hello).unwrap();
            let frame = ServerFrame {
                v: API_VERSION_MAJOR,
                reply_to: hello["id"].as_u64(),
                event: ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "fake".into(),
                    capabilities: vec![],
                },
            };
            let mut line = serde_json::to_vec(&frame).unwrap();
            line.push(b'\n');
            // Split across chunks to exercise buffering.
            let (a, b) = line.split_at(5);
            in_tx.send(a.to_vec()).unwrap();
            in_tx.send(b.to_vec()).unwrap();
            while let Some(m) = out_rx.blocking_recv() {
                if matches!(m, Outgoing::Close) {
                    break;
                }
            }
        });
        let client = JcodeClient::connect_with(
            Box::new(transport),
            ConnectOptions {
                socket_path: Some(std::path::PathBuf::new()),
                ensure_runtime: false,
                ..ConnectOptions::default()
            },
        )
        .unwrap();
        drop(client);
    }
}
