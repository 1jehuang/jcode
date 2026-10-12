use super::*;

/// Serve `body` as the OpenAI usage endpoint for every request, counting
/// requests. Returns the base URL.
pub(super) fn serve_openai_usage(
    rt: &tokio::runtime::Runtime,
    body: &'static str,
    hits: Arc<std::sync::atomic::AtomicUsize>,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind usage mock");
    let url = format!("http://{}/wham/usage", listener.local_addr().unwrap());
    rt.spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    url
}
