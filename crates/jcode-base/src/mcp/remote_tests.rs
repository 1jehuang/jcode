//! Acceptance-oriented tests: real TCP HTTP/SSE servers and the public client.
//! No transport mocks or external network services are used.
use super::*;
use crate::mcp::client::McpClient;
use crate::mcp::protocol::ContentBlock;
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

include!("remote_peer_tests.rs");
include!("remote_http_tests.rs");
include!("remote_call_tests.rs");
include!("remote_boundary_tests.rs");
include!("remote_recovery_tests.rs");
