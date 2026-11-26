//! Explicitly configured external synthesis. No redirects or implicit service discovery.
#![allow(clippy::collapsible_if)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, net::TcpListener, sync::mpsc};

    fn options(url: reqwest::Url) -> Options {
        Options::new(url, "future-model", "custom-voice", None).unwrap()
    }

    fn server(response: Vec<u8>) -> (reqwest::Url, thread::JoinHandle<serde_json::Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = endpoint(&format!("http://{}/speak", listener.local_addr().unwrap())).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") {
                    break bytes.len();
                }
            };
            let headers = String::from_utf8_lossy(&bytes);
            assert!(headers.starts_with("POST /speak HTTP/1.1"));
            let len: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            bytes.resize(header_end + len, 0);
            stream.read_exact(&mut bytes[header_end..]).unwrap();
            let body = serde_json::from_slice(&bytes[header_end..]).unwrap();
            let _ = stream.write_all(&response);
            body
        });
        (url, handle)
    }

    #[test]
    fn validates_endpoints() {
        for value in [
            "file:///tmp/a",
            "ftp://host/a",
            "http://user@host",
            "https://a/#x",
            "http://a:bad",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        assert!(endpoint("https://explicit-remote.example/tts").is_ok());
    }

    #[test]
    fn configured_service_receives_exact_openai_schema() {
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 12\r\nConnection: close\r\n\r\n".to_vec();
        response.extend_from_slice(b"RIFF\x04\0\0\0WAVE");
        let (url, server) = server(response);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(synthesize(
            &reqwest::Client::new(),
            &options(url),
            "Hello Jarvis",
        ));
        assert!(result.is_some());
        assert_eq!(
            server.join().unwrap(),
            serde_json::json!({"input":"Hello Jarvis", "model":"future-model", "voice":"custom-voice", "response_format":"wav"})
        );
    }

    #[test]
    fn openai_options_validate_without_leaking_credentials() {
        let remote = endpoint("http://remote.example/tts").unwrap();
        assert!(Options::new(remote, "model", "voice", Some("secret".into())).is_err());
        let local = endpoint("http://127.0.0.1:8765/tts").unwrap();
        assert!(Options::new(local.clone(), "", "voice", None).is_err());
        assert!(Options::new(local.clone(), "model", "voice", Some("\r\nsecret".into())).is_err());
        assert!(Options::new(local, "model", "voice", Some("secret".into())).is_ok());
        assert!(
            !SpeechError::InvalidConfiguration
                .to_string()
                .contains("secret")
        );
    }

    #[test]
    fn real_json_request_and_bounded_wav_response() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for (status, mime, body, declared, valid) in [
            (200, "audio/wav", b"RIFF\x04\0\0\0WAVE".as_slice(), 12, true),
            (
                500,
                "audio/wav",
                b"RIFF\x04\0\0\0WAVE".as_slice(),
                12,
                false,
            ),
            (
                302,
                "audio/wav",
                b"RIFF\x04\0\0\0WAVE".as_slice(),
                12,
                false,
            ),
            (200, "audio/wav", b"bad".as_slice(), 3, false),
            (
                200,
                "text/plain",
                b"RIFF\x04\0\0\0WAVE".as_slice(),
                12,
                false,
            ),
            (200, "audio/wav", b"".as_slice(), MAX_WAV + 1, false),
        ] {
            let mut response = format!("HTTP/1.1 {status} Test\r\nContent-Type: {mime}\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n").into_bytes();
            response.extend_from_slice(body);
            let (url, handle) = server(response);
            assert_eq!(
                runtime
                    .block_on(synthesize(&client, &options(url), "hello \"world\""))
                    .is_some(),
                valid
            );
            assert_eq!(
                handle.join().unwrap(),
                serde_json::json!({"input": "hello \"world\"", "model": "future-model", "voice": "custom-voice", "response_format": "wav"})
            );
        }
    }

    #[test]
    fn streaming_body_without_length_is_bounded() {
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n".to_vec();
        response.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        response.resize(response.len() + MAX_WAV, 0);
        let (url, server) = server(response);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(synthesize(
                    &reqwest::Client::new(),
                    &options(url),
                    "bounded"
                ))
                .is_none()
        );
        server.join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn speech_beginning_during_synthesis_blocks_prepared_audio_until_authorized() {
        for revise in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = endpoint(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            let (synth, started) = mpsc::channel();
            let (finish, ready) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0; 4096];
                assert!(socket.read(&mut buf).unwrap() > 0);
                synth.send(()).unwrap();
                ready.recv_timeout(Duration::from_secs(5)).unwrap();
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 12\r\nConnection: close\r\n\r\nRIFF\x04\0\0\0WAVE").unwrap();
            });
            let (spawned, played) = mpsc::channel();
            let player = create_with_player(
                options(url),
                Arc::new(move |_| {
                    spawned.send(()).unwrap();
                    Command::new("cat")
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .spawn()
                }),
            )
            .unwrap();
            player
                .enqueue_gated(
                    "test",
                    super::super::PlaybackTrace {
                        session: "test".into(),
                        utterance: 1,
                    },
                )
                .unwrap();
            started.recv_timeout(Duration::from_secs(5)).unwrap();
            // It was quiet when synthesis started. Audio isn't ready, so no
            // mic claim can occur. The user starts speaking before WAV arrives.
            assert!(!player.authorize_ready(|| panic!("suppressed mic during synthesis")));
            assert!(!player.owns_audio_output());
            finish.send(()).unwrap();
            server.join().unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !player.shared.0.lock().unwrap().awaiting_quiet {
                assert!(Instant::now() < deadline);
                thread::sleep(POLL);
            }
            assert!(!player.authorize_ready(|| false));
            assert!(!player.owns_audio_output());
            assert!(played.try_recv().is_err());
            if revise {
                player.cancel();
                drop(player);
                assert!(played.try_recv().is_err());
            } else {
                assert!(player.authorize_ready(|| true));
                played.recv_timeout(Duration::from_secs(3)).unwrap();
            }
        }
    }

    #[test]
    fn blocked_http_does_not_block_enqueue_cancel_or_drop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = endpoint(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buf = [0; 4096];
            assert!(socket.read(&mut buf).unwrap() > 0);
            tx.send(()).unwrap();
            // Wait for cancellation to close the request, not for a 30-second timeout.
            loop {
                if socket.read(&mut buf).unwrap() == 0 {
                    break;
                }
            }
        });
        let player =
            create_with_player(options(url), Arc::new(|_| panic!("cancelled audio played")))
                .unwrap();
        player.enqueue("blocked").unwrap();
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let start = Instant::now();
        for _ in 0..MAX_QUEUED_UTTERANCES {
            player.enqueue("pending").unwrap();
        }
        assert_eq!(player.enqueue("overflow"), Err(SpeechError::QueueFull));
        player.cancel();
        drop(player);
        assert!(start.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }

    #[test]
    fn asynchronous_failure_is_reported_without_secret_or_url() {
        let response = b"HTTP/1.1 401 Denied\r\nContent-Length: 0\r\n\r\n".to_vec();
        let (url, server) = server(response);
        let player =
            create_with_player(options(url), Arc::new(|_| panic!("failed request played")))
                .unwrap();
        player.enqueue("test").unwrap();
        let start = Instant::now();
        while !player.is_idle() {
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(POLL);
        }
        assert_eq!(
            player.take_error(),
            Some("Speech API request failed or returned invalid WAV audio")
        );
        assert_eq!(player.take_error(), None);
        server.join().unwrap();
    }

    /// Explicit local-only acceptance check. Requires a running Piper service
    /// and audio output, so it is never run by the hosted suite.
    #[test]
    #[ignore = "requires local Piper service and audible output"]
    fn real_local_piper_api_playback() {
        let endpoint =
            std::env::var("JCODE_TEST_SPEECH_ENDPOINT").expect("set test speech endpoint");
        let player = SpeechPlayer::with_http(&endpoint, "piper-jarvis", "jarvis", None).unwrap();
        player
            .enqueue(
                "Jarvis is online. This reply is generated locally through Jcode's speech API.",
            )
            .unwrap();
        let start = Instant::now();
        while !player.is_idle() {
            assert!(start.elapsed() < Duration::from_secs(30));
            thread::sleep(POLL);
        }
        assert_eq!(player.take_error(), None);
        println!(
            "Real Piper speech API and local playback completed in {:?}",
            start.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn http_audio_reaches_fake_player_without_sound() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 12\r\n\r\nRIFF\x04\0\0\0WAVE".to_vec();
        let (url, server) = server(response);
        let output = tempfile::NamedTempFile::new().unwrap();
        let file = output.reopen().unwrap();
        let player = create_with_player(
            options(url),
            Arc::new(move |_| {
                Command::new("cat")
                    .stdin(Stdio::piped())
                    .stdout(file.try_clone().unwrap())
                    .spawn()
            }),
        )
        .unwrap();
        player.enqueue("hello").unwrap();
        let start = Instant::now();
        while std::fs::metadata(output.path()).unwrap().len() != 12 {
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(POLL);
        }
        assert_eq!(std::fs::read(output.path()).unwrap(), b"RIFF\x04\0\0\0WAVE");
        drop(player);
        server.join().unwrap();
    }
}
use super::*;
use std::io::Write;

const MAX_WAV: usize = 16 * 1024 * 1024;
type Shared = Arc<(Mutex<State>, Condvar)>;

fn endpoint(value: &str) -> Result<reqwest::Url, SpeechError> {
    let url = reqwest::Url::parse(value).map_err(|_| SpeechError::InvalidEndpoint)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(SpeechError::InvalidEndpoint);
    }
    Ok(url)
}

pub(super) fn create(
    value: &str,
    model: &str,
    voice: &str,
    api_key: Option<String>,
) -> Result<SpeechPlayer, SpeechError> {
    create_player(Options::new(endpoint(value)?, model, voice, api_key)?)
}

fn create_player(options: Options) -> Result<SpeechPlayer, SpeechError> {
    let executable = ["pw-play", "aplay"]
        .into_iter()
        .find(|name| probe(name))
        .ok_or(SpeechError::PlayerUnavailable)?;
    create_with_player(
        options,
        Arc::new(move |_| {
            Command::new(executable)
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        }),
    )
}

struct Options {
    url: reqwest::Url,
    model: String,
    voice: String,
    authorization: Option<reqwest::header::HeaderValue>,
}

impl Options {
    fn new(
        url: reqwest::Url,
        model: &str,
        voice: &str,
        api_key: Option<String>,
    ) -> Result<Self, SpeechError> {
        if [model, voice]
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(SpeechError::InvalidConfiguration);
        }
        let authorization = match api_key {
            Some(key) if !key.trim().is_empty() => {
                // Provider credentials must not travel over remote plain HTTP.
                let loopback = url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                if url.scheme() != "https" && !loopback {
                    return Err(SpeechError::InvalidConfiguration);
                }
                let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|_| SpeechError::InvalidConfiguration)?;
                header.set_sensitive(true);
                Some(header)
            }
            Some(_) => return Err(SpeechError::InvalidConfiguration),
            None => None,
        };
        Ok(Self {
            url,
            model: model.into(),
            voice: voice.into(),
            authorization,
        })
    }
}

fn create_with_player(options: Options, backend: Backend) -> Result<SpeechPlayer, SpeechError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| SpeechError::WorkerUnavailable)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| SpeechError::WorkerUnavailable)?;
    let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
    let state = shared.clone();
    let worker = thread::Builder::new()
        .name("jcode-http-speech".into())
        .spawn(move || {
            run_http(state, backend, client, options, runtime);
        })
        .map_err(|_| SpeechError::WorkerUnavailable)?;
    Ok(SpeechPlayer {
        shared,
        worker: Some(worker),
    })
}

async fn synthesize(client: &reqwest::Client, options: &Options, text: &str) -> Option<Vec<u8>> {
    let body = serde_json::json!({"model": options.model, "voice": options.voice, "input": text, "response_format": "wav"});
    let mut request = client.post(options.url.clone()).json(&body);
    if let Some(authorization) = &options.authorization {
        request = request.header(reqwest::header::AUTHORIZATION, authorization.clone());
    }
    let mut response = request.send().await.ok()?;
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)?
            .to_str()
            .ok()?
            .split(';')
            .next()?
            .trim()
            != "audio/wav"
        || response
            .content_length()
            .is_some_and(|n| n > MAX_WAV as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > MAX_WAV - bytes.len() {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    Some(bytes)
}

fn run_http(
    shared: Shared,
    backend: Backend,
    client: reqwest::Client,
    options: Options,
    runtime: tokio::runtime::Runtime,
) {
    let (lock, wake) = &*shared;
    loop {
        let ((text, trace), generation) = {
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if state.stopped {
                    return;
                }
                if let Some((child, start)) = state.active.as_mut() {
                    match child.try_wait() {
                        Ok(None) if start.elapsed() < MAX_PLAYBACK => {}
                        Ok(Some(status)) if status.success() => {
                            crate::timing::mark("speech playback complete");
                            state.stop_child();
                        }
                        _ => {
                            state.last_error =
                                Some("Local speech audio playback failed or timed out");
                            state.stop_child();
                        }
                    }
                }
                if state.active.is_none() {
                    if let Some(text) = state.queue.pop_front() {
                        state.synthesizing = true;
                        break (text, state.generation);
                    }
                }
                state = wake
                    .wait_timeout(state, POLL)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        };
        crate::timing::mark("speech synthesis started");
        if let Some(trace) = &trace {
            trace.mark("synthesis_started", 0);
        }
        let wav = runtime.block_on(async {
            tokio::select! {
                result = synthesize(&client, &options, &text) => result,
                _ = async {
                    loop {
                        tokio::time::sleep(POLL).await;
                        let state = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if state.stopped || state.generation != generation { break; }
                    }
                } => None,
            }
        });
        let Some(wav) = wav else {
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            state.synthesizing = false;
            if !state.stopped && state.generation == generation {
                state.last_error = Some("Speech API request failed or returned invalid WAV audio");
            }
            continue;
        };
        let stdin = {
            let state = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (mut state, authorized) =
                super::await_quiet(state, wake, generation, trace.is_some());
            state.synthesizing = false;
            if !authorized {
                continue;
            }
            // Spawn is serialized with cancel, but network and pipe writes never hold the lock.
            let mut child = match backend(&text) {
                Ok(child) => child,
                Err(_) => {
                    state.last_error = Some("Could not start the local speech audio player");
                    continue;
                }
            };
            let stdin = child.stdin.take();
            if let Some(trace) = &trace {
                trace.mark("player_started", 0);
            }
            crate::timing::mark("speech playback process started");
            state.active = Some((child, Instant::now()));
            stdin
        };
        if let Some(mut stdin) = stdin {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                // A stalled player must not trap the worker in a full pipe.
                let fd = stdin.as_raw_fd();
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                if flags < 0
                    || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
                {
                    lock.lock().unwrap_or_else(|e| e.into_inner()).stop_child();
                    continue;
                }
            }
            let mut remaining = wav.as_slice();
            while !remaining.is_empty() {
                {
                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if state.stopped || state.generation != generation {
                        break;
                    }
                    if state
                        .active
                        .as_ref()
                        .is_some_and(|(_, start)| start.elapsed() >= MAX_PLAYBACK)
                    {
                        state.stop_child();
                        break;
                    }
                }
                match stdin.write(remaining) {
                    Ok(0) => break,
                    Ok(n) => remaining = &remaining[n..],
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
    }
}
