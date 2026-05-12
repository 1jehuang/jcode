//! jcode-grpc: gRPC Server for LLM Services
//!
//! ## Overview
//!
//! This crate provides a gRPC server implementation for the LLM service defined in jcode.proto.
//! It integrates with the jcode-llm provider layer to support:
//!
//! - **Deepseek**: Cloud-based LLM API
//! - **vLLM**: High-throughput local serving
//! - **llama.cpp**: Lightweight local inference
//! - **OpenAI Compatible**: Any OpenAI-compatible endpoint
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │  gRPC Client    │────▶│  LlmServiceImpl  │────▶│  LlmProvider    │
//! │  (Cursor/IDE)   │     │  (gRPC Server)   │     │  (Deepseek/vLLM)│
//! └─────────────────┘     └──────────────────┘     └─────────────────┘
//!                                 │
//!                                 ▼
//!                        ┌──────────────────┐
//!                        │  RAG Integration │
//!                        │  (editing_layer) │
//!                        └──────────────────┘
//! ```

pub mod server;
pub mod streaming;
pub mod rag_integration;
pub mod error_handling;

pub use server::LlmServiceImpl;
pub use rag_integration::{RagLlmService, RagChatContext};
pub use error_handling::{LlmErrorCode, ErrorMetadata};
