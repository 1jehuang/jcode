//! A standalone Muse Code (MSP) client for jcode.
//!
//! # Why this exists
//!
//! Meta draws a hard line between two ways to reach Muse models:
//!
//! - **Model API** (pay-as-you-go), which jcode already reaches through the
//!   `meta-muse` OpenAI-compatible profile.
//! - **Muse Code**, a terminal agent whose subscription credential Meta
//!   describes as "for use with Muse Code only", with additional API keys billed
//!   pay-as-you-go.
//!
//! So a Muse Code subscriber gets no benefit from the existing `meta-muse`
//! provider. This crate is the foundation for closing that gap: a client for the
//! Muse Session Protocol (MSP), which Meta publishes an MIT-licensed SDK for
//! specifically so third parties can "drive Muse Code agent sessions".
//!
//! # Compliance boundary
//!
//! **This crate never reads the subscription credential.** That credential lives
//! in `~/.config/muse/auth.json` and stays there; the `muse` process owns it and
//! owns every model call. This crate only speaks MSP to that process over stdio.
//!
//! The distinction matters and is deliberate. The credential *is* accepted by
//! `api.meta.ai` if read out and reused, so the restriction is contractual
//! rather than technical; driving the official process is what keeps usage on
//! the licence that permits it.
//!
//! # Why this is the first piece
//!
//! MSP is a large, versioned protocol (v1.4.10 exposes 69 methods, 41
//! notifications and 37 errors) with several behaviours that the published
//! schema does not describe. Getting those right is the highest-risk, most
//! self-contained part of the work, so it lands first and independently of any
//! decision about how jcode should surface a foreign agent's loop.
//!
//! See [`msp`] for the protocol details, including the three handshake rules
//! that are not in the schema and the idempotency requirement on every mutation.

pub mod msp;

pub use msp::{
    ClassifiedFrame, MspError, MspInbound, MspTransport, MuseHostConfig, is_valid_client_name,
    new_command_id,
};

/// Stable machine-facing provider id.
pub const PROVIDER_ID: &str = "muse-code";

/// Human-facing provider label.
pub const PROVIDER_LABEL: &str = "Muse Code";

/// Client identity sent in `initialize`. MSP requires `^[a-z0-9_]+$`.
pub const MSP_CLIENT_NAME: &str = "jcode_muse_code";

/// MSP client version advertised in `initialize`.
pub const MSP_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
