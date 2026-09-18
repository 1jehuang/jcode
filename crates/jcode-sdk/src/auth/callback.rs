//! Bounded local OAuth redirect receiver. No credential or URL is logged.
use super::*;
use std::net::{Ipv4Addr, TcpListener, TcpStream};

pub(super) struct CallbackListener {
    listener: TcpListener,
    redirect: url::Url,
    state: String,
}

impl CallbackListener {
    pub(super) fn bind(auth_url: &url::Url) -> Option<Self> {
        let unique = |name| {
            let mut values = auth_url.query_pairs().filter(|(key, _)| key == name);
            let value = values.next()?.1.into_owned();
            values.next().is_none().then_some(value)
        };
        let redirect = url::Url::parse(&unique("redirect_uri")?).ok()?;
        let state = unique("state")?;
        if redirect.scheme() != "http"
            || !matches!(redirect.host_str(), Some("localhost" | "127.0.0.1"))
            || !redirect.username().is_empty()
            || redirect.password().is_some()
            || redirect.query().is_some()
            || redirect.fragment().is_some()
            || state.is_empty()
        {
            return None;
        }
        let port = redirect.port()?;
        if port == 0 {
            return None;
        }
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).ok()?;
        // Unix TIME_WAIT must not disable the fixed OAuth port for the next
        // login. Do not use SO_REUSEPORT or Windows SO_REUSEADDR, which could
        // allow a second active listener to intercept callbacks.
        #[cfg(unix)]
        socket.set_reuse_address(true).ok()?;
        socket
            .bind(&std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())
            .ok()?;
        socket.listen(16).ok()?;
        let listener: TcpListener = socket.into();
        listener.set_nonblocking(true).ok()?;
        Some(Self {
            listener,
            redirect,
            state,
        })
    }

    fn interrupted(flow: &FlowInner) -> bool {
        flow.cancelled.load(Ordering::Acquire) || flow.finished.load(Ordering::Acquire)
    }

    pub(super) fn wait(&self, flow: &FlowInner) -> Result<String> {
        let deadline = Instant::now() + flow.options.timeout;
        loop {
            if Self::interrupted(flow) {
                return Err(cancelled());
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorKind::Timeout,
                    "Browser callback timed out. Paste the callback URL or start a new login.",
                ));
            }
            match self.listener.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    if let Some(input) = self.receive(&mut stream, flow, deadline) {
                        return Ok(input);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => {
                    return Err(failed(
                        "Could not receive browser callback. Paste the callback URL instead.",
                    ));
                }
            }
        }
    }

    fn receive(
        &self,
        stream: &mut TcpStream,
        flow: &FlowInner,
        deadline: Instant,
    ) -> Option<String> {
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .ok()?;
        stream
            .set_write_timeout(Some(Duration::from_millis(100)))
            .ok()?;
        let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
        let mut request = Vec::new();
        let input = loop {
            if Self::interrupted(flow) || Instant::now() >= deadline {
                return None;
            }
            let mut buf = [0; 1024];
            match stream.read(&mut buf) {
                Ok(0) => break None,
                Ok(n) => request.extend_from_slice(&buf[..n]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => break None,
            }
            if request.len() > INPUT_LIMIT {
                break None;
            }
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                break self.parse_request(&request);
            }
        };
        let (status, body) = if input.is_some() {
            (
                "200 OK",
                "Authorization received. Return to Jcode to finish signing in.",
            )
        } else {
            (
                "400 Bad Request",
                "This callback does not match the pending Jcode login.",
            )
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        input
    }

    fn parse_request(&self, request: &[u8]) -> Option<String> {
        let request = std::str::from_utf8(request).ok()?;
        let mut words = request.lines().next()?.split_whitespace();
        if words.next()? != "GET" {
            return None;
        }
        let target = words.next()?;
        if !target.starts_with('/') || target.starts_with("//") || target.contains('#') {
            return None;
        }
        if !matches!(words.next(), Some("HTTP/1.0" | "HTTP/1.1")) || words.next().is_some() {
            return None;
        }
        let url = url::Url::parse(&format!(
            "{}{}",
            self.redirect.origin().ascii_serialization(),
            target
        ))
        .ok()?;
        if url.origin() != self.redirect.origin() || url.path() != self.redirect.path() {
            return None;
        }
        let mut states = url.query_pairs().filter(|(key, _)| key == "state");
        if states.next()?.1 != self.state || states.next().is_some() {
            return None;
        }
        let fields: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key == "code" || key == "error")
            .collect();
        if fields.len() != 1 || fields[0].1.is_empty() {
            return None;
        }
        Some(url.to_string())
    }
}
