//! JBench — jcode's git-commit-reconstruction evaluation framework.
//!
//! This crate is a scaffold: data types are real and roundtrip-tested,
//! but orchestration logic is stubbed with `unimplemented!()` so that
//! reviewers can validate the public API surface before behavior lands.
//!
//! See `README.md` for the design and the BuffBench reference at
//! `/tmp/codebuff/evals/buffbench/` for the TypeScript original.
//!
//! The crate consumes [`jcode_agent_runtime::AgentRegistry`] and
//! [`jcode_agent_runtime::AgentDefinition`] for agent discovery and
//! configuration; it does not redefine those concepts locally.

#![forbid(unsafe_code)]

pub mod agent_runner;
pub mod judge;
pub mod lessons;
pub mod types;
