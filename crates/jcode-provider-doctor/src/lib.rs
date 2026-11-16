//! Provider-doctor diagnostics for jcode.
//!
//! Sits downstream of `jcode-base` so edits to the doctor cluster do not
//! rebuild the base -> app-core -> tui spine:
//! - [`provider_e2e`]: the strict end-to-end runner behind `jcode
//!   provider-doctor` (offline/catalog/full tiers, native + OpenAI-compatible
//!   drivers).
//! - [`live_provider_probes`]: the live HTTP/native-runtime probes the doctor
//!   drives (models fetch, chat, streaming, tool-call smokes).
//! - `lifecycle_driver` (test-only): the auth-lifecycle contract driver and
//!   its provider matrices.

pub mod live_provider_probes;
pub mod provider_e2e;

mod credential_source;

// The driver's items are exercised only by its internal #[cfg(test)] tests;
// nothing outside this crate consumes it.
#[cfg(test)]
mod lifecycle_driver;

pub use provider_e2e::{
    DoctorCheck, DoctorReport, DoctorSpend, DoctorTier, NativeProviderKind,
    native_doctor_supports_provider, run_antigravity_native_e2e, run_claude_native_e2e,
    run_generic_native_e2e, run_provider_e2e,
};
