//! `jbench` CLI entry point.
//!
//! This is a scaffold: every subcommand prints a TODO line describing
//! the work it will do and exits 0. The argument shape, however, is
//! real and stable — downstream tooling (CI, scripts) can wire against
//! these subcommands today and pick up real behavior as Phases 5.3 →
//! 5.5 land.
//!
//! All real work happens through the [`jcode_jbench`] library; this
//! binary's only job is to dispatch.

use clap::{Parser, Subcommand};

// Pull in the library so the binary depends on it (and fails to
// compile if its public surface regresses).
use jcode_jbench as _;

/// Top-level `jbench` CLI.
#[derive(Debug, Parser)]
#[command(
    name = "jbench",
    about = "JBench — jcode's git-commit-reconstruction eval framework",
    version
)]
struct Cli {
    /// Subcommand to dispatch to.
    #[command(subcommand)]
    command: Command,
}

/// JBench subcommands. Each is a stub today; see `README.md` for the
/// intended workflow.
#[derive(Debug, Subcommand)]
enum Command {
    /// Select high-quality commits from a target repo to use as eval
    /// tasks.
    PickCommits,
    /// Generate an `eval-{repo}.json` file (`EvalDataV2`) from a list
    /// of picked commits.
    GenEvals,
    /// Run one or more agents against an eval data file and emit
    /// per-commit `EvalRun`s.
    Run,
    /// Re-judge an existing run with the three-judge median pipeline.
    Judge,
    /// Aggregate and analyze results across all tasks for an agent.
    MetaAnalyze,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::PickCommits => {
            println!("TODO: jbench pick-commits — Phase 5.2 will implement commit selection.");
        }
        Command::GenEvals => {
            println!("TODO: jbench gen-evals — Phase 5.2 will implement eval-data generation.");
        }
        Command::Run => {
            println!("TODO: jbench run — Phase 5.3 will implement agent_runner orchestration.");
        }
        Command::Judge => {
            println!("TODO: jbench judge — Phase 5.4 will implement three-judge median scoring.");
        }
        Command::MetaAnalyze => {
            println!("TODO: jbench meta-analyze — Phase 5.6 will implement cross-task aggregation.");
        }
    }
}
