//! WebSocket connector for the OpenAI Responses API with `permessage-deflate`.
//!
//! `tokio_tungstenite::connect_async` cannot negotiate compression (tungstenite
//! has no extension support and rejects RSV1 frames). This connector does the
//! TCP + TLS setup itself, with the same rustls + native-roots configuration
//! tokio-tungstenite used, then runs the WebSocket handshake over a
//! [`DeflateStream`] wrapped around the TLS stream so the compression layer
//! sees plaintext WebSocket frames.
//!
//! Both `chatgpt.com` and `api.openai.com` accept the extension (verified
//! live). With `[provider] request_compression = "off"` the extension is not
//! offered and the layer is a byte-for-byte pass-through.

use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context as TaskContext, Poll};

use anyhow::Result;
use jcode_provider_core::request_compression::CompressionMode;
use jcode_provider_core::ws_deflate::{DeflateStream, PERMESSAGE_DEFLATE_OFFER};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::{TlsError, UrlError};
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::tungstenite::http::HeaderValue;

/// Transport under the WebSocket codec: plain TCP (`ws://`, tests and local
/// proxies) or rustls (`wss://`).
pub(crate) enum WsTransport {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

/// The socket type every OpenAI WebSocket path uses.
pub(crate) type OpenAIWs = WebSocketStream<DeflateStream<WsTransport>>;

impl AsyncRead for WsTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for WsTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Native-root rustls config, built once (loading the system store is slow).
fn tls_config() -> Result<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<std::result::Result<Arc<rustls::ClientConfig>, String>> =
        OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let loaded = rustls_native_certs::load_native_certs();
            if !loaded.errors.is_empty() {
                jcode_base::logging::warn(&format!(
                    "openai websocket: native root certificate load errors: {:?}",
                    loaded.errors
                ));
            }
            let mut roots = rustls::RootCertStore::empty();
            let (added, _) = roots.add_parsable_certificates(loaded.certs);
            if added == 0 {
                return Err("no native root CA certificates found".to_string());
            }
            Ok(Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ))
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

/// Connect a WebSocket, offering `permessage-deflate` unless `mode` is `Off`.
///
/// Errors are tungstenite's own [`WsError`] so callers keep classifying
/// upgrade failures (for example HTTP 426) exactly as with `connect_async`.
pub(crate) async fn connect_ws_with_mode<R>(
    request: R,
    mode: CompressionMode,
) -> Result<(OpenAIWs, Response), WsError>
where
    R: IntoClientRequest + Unpin,
{
    let mut request = request.into_client_request()?;
    if mode != CompressionMode::Off {
        request.headers_mut().insert(
            "Sec-WebSocket-Extensions",
            HeaderValue::from_static(PERMESSAGE_DEFLATE_OFFER),
        );
    }
    let uri = request.uri().clone();
    let host = uri
        .host()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string())
        .ok_or(WsError::Url(UrlError::NoHostName))?;
    let tls = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(WsError::Url(UrlError::UnsupportedUrlScheme)),
    };
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port)).await?;
    let transport = if tls {
        let server_name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| WsError::Tls(TlsError::InvalidDnsName))?;
        let config = tls_config()
            .map_err(|err| WsError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, err)))?;
        let stream = tokio_rustls::TlsConnector::from(config)
            .connect(server_name, tcp)
            .await?;
        WsTransport::Tls(Box::new(stream))
    } else {
        WsTransport::Plain(tcp)
    };
    let (ws, response) =
        tokio_tungstenite::client_async(request, DeflateStream::new(transport, mode)).await?;
    if ws.get_ref().deflate_negotiated() {
        jcode_base::logging::info(&format!(
            "openai websocket: permessage-deflate negotiated with {host}"
        ));
    }
    Ok((ws, response))
}

/// Connect a WebSocket using the configured `[provider] request_compression`.
pub(crate) async fn connect_ws<R>(request: R) -> Result<(OpenAIWs, Response), WsError>
where
    R: IntoClientRequest + Unpin,
{
    connect_ws_with_mode(request, jcode_base::provider::request_compression_mode()).await
}
