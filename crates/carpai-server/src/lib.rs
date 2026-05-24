//! CarpAI Enterprise Server
//!
//! Multi-tenant AI coding assistant server with gRPC, REST, and WebSocket APIs.

pub mod config;
pub mod app;
pub mod grpc;
pub mod rest;
pub mod ws;
pub mod auth;
pub mod enterprise;
pub mod observability;
pub mod service;

pub use config::ServerConfig;
pub use app::Application;
