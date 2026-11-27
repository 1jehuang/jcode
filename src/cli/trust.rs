//! `jcode trust`: review and approve what a project wants to run.

use super::args::{Command, TrustCommand, TrustScopeArg};
use crate::project_trust::{self, Scope, ScopeReview, TrustState};
use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

fn can_prompt() -> bool {
    io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && std::env::var_os("JCODE_NON_INTERACTIVE").is_none()
}

fn scopes_for(arg: TrustScopeArg) -> Vec<Scope> {
    match arg {
        TrustScopeArg::Hooks => vec![Scope::Hooks],
        TrustScopeArg::Mcp => vec![Scope::Mcp],
        TrustScopeArg::Agents => vec![Scope::Agents],
        TrustScopeArg::All => Scope::ALL.to_vec(),
    }
}

/// What each scope would let `path`'s project run. Scopes with nothing to
/// approve are omitted. Each feature that adds a scope adds its review here.
fn scope_reviews(path: &Path) -> Result<Vec<ScopeReview>> {
    let mut reviews = Vec::new();
    if let Some(review) = crate::mcp::mcp_scope_review(path)? {
        reviews.push(review);
    }
    Ok(reviews)
}

fn describe(scope: Scope) -> &'static str {
    match scope {
        Scope::Hooks => "hook command(s) that run on every matching event",
        Scope::Mcp => "MCP server(s) that can run local commands",
        Scope::Agents => "agent definition(s) that set model, effort or tools",
    }
}

fn show_review(review: &ScopeReview) {
    eprintln!();
    eprintln!(
        "[{}] {} {}:",
        review.scope.as_str(),
        review.item_count,
        describe(review.scope)
    );
    for line in &review.lines {
        eprintln!("{line}");
    }
    if review.redacted {
        eprintln!("Some environment values are redacted and may affect execution.");
    }
}

fn resolve(path: Option<PathBuf>) -> Result<PathBuf> {
    match path {
        Some(path) => Ok(path),
        None => Ok(std::env::current_dir()?),
    }
}

fn prompt_for_approval(scope: Scope) -> Result<bool> {
    eprintln!(
        "Trust this exact {} configuration for future sessions? Any change requires approval again.",
        scope.as_str()
    );
    eprint!("Approve? [y/N]: ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn show(path: Option<PathBuf>) -> Result<()> {
    let path = resolve(path)?;
    let root = project_trust::project_root(&path)?;
    println!(
        "Project: {}",
        project_trust::safe_terminal_text(&root.to_string_lossy())
    );
    let reviews = scope_reviews(&path)?;
    if reviews.is_empty() {
        println!("Nothing in this project needs approval.");
        return Ok(());
    }
    for review in &reviews {
        let state = project_trust::trust_state(&path, review.scope, &review.digest)?;
        println!(
            "{}: {} ({} item(s), digest {})",
            review.scope.as_str(),
            state.label(),
            review.item_count,
            review.digest
        );
        show_review(review);
    }
    println!("Approve with `jcode trust grant`.");
    Ok(())
}

fn grant(path: Option<PathBuf>, scope: TrustScopeArg, yes: bool) -> Result<()> {
    let path = resolve(path)?;
    let wanted = scopes_for(scope);
    let reviews: Vec<_> = scope_reviews(&path)?
        .into_iter()
        .filter(|review| wanted.contains(&review.scope))
        .collect();
    if reviews.is_empty() {
        println!("Nothing to approve for the selected scope(s).");
        return Ok(());
    }
    for review in reviews {
        let state = project_trust::trust_state(&path, review.scope, &review.digest)?;
        if state == TrustState::Trusted {
            println!("{}: already trusted.", review.scope.as_str());
            continue;
        }
        show_review(&review);
        let approved = if yes {
            true
        } else if can_prompt() {
            if review.redacted {
                anyhow::bail!(
                    "approval is disabled because environment values were redacted; review the project files and referenced environment values, then rerun with --yes"
                );
            }
            prompt_for_approval(review.scope)?
        } else {
            anyhow::bail!(
                "project trust requires confirmation; rerun in an interactive terminal or pass --yes after reviewing `jcode trust show`"
            );
        };
        if approved {
            project_trust::grant(&path, review.scope, &review.digest)?;
            println!(
                "{}: trusted {} item(s). Any change requires approval again.",
                review.scope.as_str(),
                review.item_count
            );
        } else {
            println!("{}: remains blocked.", review.scope.as_str());
        }
    }
    Ok(())
}

fn revoke(path: Option<PathBuf>, scope: TrustScopeArg) -> Result<()> {
    let path = resolve(path)?;
    let mut removed_any = false;
    if scope == TrustScopeArg::All {
        removed_any = project_trust::revoke(&path, None)?;
    } else {
        for scope in scopes_for(scope) {
            removed_any |= project_trust::revoke(&path, Some(scope))?;
        }
    }
    if removed_any {
        println!("Removed saved project trust.");
    } else {
        println!("No saved project trust to remove.");
    }
    Ok(())
}

fn list() {
    let entries = project_trust::list();
    if entries.is_empty() {
        println!("No projects are trusted.");
        return;
    }
    for (project, scope, trust) in entries {
        println!(
            "{}\t{}\t{}\t{}",
            project_trust::safe_terminal_text(&project),
            scope.as_str(),
            trust.trusted_at,
            trust.digest
        );
    }
}

pub(crate) fn is_offline_command(command: Option<&Command>) -> bool {
    matches!(command, Some(Command::Mcp { .. } | Command::Trust { .. }))
}

/// `jcode mcp ...` and `jcode trust ...`: independent of provider profiles,
/// sessions and the daemon. Any other command is a no-op.
pub(crate) async fn run_offline_command(command: Option<Command>) -> Result<()> {
    match command {
        Some(Command::Mcp { action }) => super::mcp::run(action).await,
        Some(Command::Trust { action }) => run_trust_command(action),
        _ => Ok(()),
    }
}

pub(crate) fn run_trust_command(action: TrustCommand) -> Result<()> {
    match action {
        TrustCommand::Show { path } => show(path),
        TrustCommand::Grant { path, scope, yes } => grant(path, scope, yes),
        TrustCommand::Revoke { path, scope } => revoke(path, scope),
        TrustCommand::List => {
            list();
            Ok(())
        }
    }
}

/// At the start of an interactive session, offer to approve untrusted project
/// scopes. Headless runs never prompt and never block: untrusted scopes are
/// simply skipped by their loaders.
pub(crate) fn maybe_prompt_for_project_trust(command: Option<&Command>) -> Result<()> {
    // Only commands that start an interactive session offer to approve a project.
    let interactive = matches!(
        command,
        None | Some(Command::Connect) | Some(Command::Repl) | Some(Command::SelfDev { .. })
    );
    if !interactive || !can_prompt() {
        return Ok(());
    }
    let path = std::env::current_dir()?;
    for review in scope_reviews(&path)? {
        if project_trust::trust_state(&path, review.scope, &review.digest)? == TrustState::Trusted {
            continue;
        }
        show_review(&review);
        eprintln!("Jcode has not started or run any of this.");
        if review.redacted {
            eprintln!(
                "Skipped automatic approval. Review the project files and referenced environment values, then run `jcode trust grant --scope {} --yes`.",
                review.scope.as_str()
            );
            continue;
        }
        if prompt_for_approval(review.scope)? {
            project_trust::grant(&path, review.scope, &review.digest)?;
            eprintln!("Trusted project {} configuration.", review.scope.as_str());
        } else {
            eprintln!(
                "Skipped. Run `jcode trust grant --scope {}` here to review it later.",
                review.scope.as_str()
            );
        }
    }
    Ok(())
}
