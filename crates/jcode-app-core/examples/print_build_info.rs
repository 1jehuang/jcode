//! Print the build manifest and current-version metadata.
//!
//! `jcode_app_core::build` re-exports the build metadata the
//! `jcode-build-support` crate computes at compile time (git hash,
//! build time, current version) and at runtime (current build
//! symlink, stable vs canary status). All of the harness / HERDR
//! code that says "this is the running binary's identity" reads
//! from this surface.
//!
//! Run with: `cargo run -p jcode-app-core --example print_build_info`
//!
//! All build-info functions take a repo dir (or none, for the
//! `current_version_file()` static path) and return `Result<_>`;
//! the example uses the current working directory as the repo dir
//! and unwraps to keep the call sites readable.

use std::path::Path;

use jcode_app_core::build::{
    current_build_info, current_git_diff, current_git_hash, current_git_hash_full,
    current_source_state, current_version_file, is_working_tree_dirty,
};

fn main() {
    let repo = Path::new(".");

    let info = current_build_info(repo).expect("current_build_info");
    println!("current_build_info:");
    println!("  hash:           {}", info.hash);
    println!("  full_hash:      {}", info.full_hash);
    println!("  built_at:       {}", info.built_at);
    println!("  commit_message: {:?}", info.commit_message);
    println!("  dirty:          {}", info.dirty);
    println!("  version_label:  {:?}", info.version_label);
    println!("  source_fingerprint: {:?}", info.source_fingerprint);

    println!();
    println!(
        "current_git_hash:        {}",
        current_git_hash(repo).expect("current_git_hash")
    );
    println!(
        "current_git_hash_full:   {}",
        current_git_hash_full(repo).expect("current_git_hash_full")
    );
    println!(
        "current_git_diff:        {}",
        current_git_diff(repo).expect("current_git_diff")
    );
    println!(
        "current_source_state:    {:?}",
        current_source_state(repo).expect("current_source_state")
    );
    println!(
        "is_working_tree_dirty:   {}",
        is_working_tree_dirty(repo).expect("is_working_tree_dirty")
    );
    println!(
        "current_version_file:    {:?}",
        current_version_file().expect("current_version_file")
    );
}
