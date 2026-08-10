use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use quote::ToTokens;

fn main() {
    let mut files = Vec::new();
    rust_files(Path::new("src"), &mut files);
    files.sort();

    let mut state = std::collections::hash_map::DefaultHasher::new();
    let mut build = std::collections::hash_map::DefaultHasher::new();
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        path.hash(&mut build);
        let bytes =
            std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        bytes.hash(&mut build);

        // App retains instances of types from across the current Scene/Model
        // modules. Hash all non-test data declarations, not just the two root
        // structs, so a nested layout change cannot cross the dynamic boundary.
        if !path.components().any(|part| part.as_os_str() == "tests") {
            let source = std::str::from_utf8(&bytes)
                .unwrap_or_else(|error| panic!("utf8 {}: {error}", path.display()));
            let syntax = syn::parse_file(source)
                .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
            hash_state_items(&syntax.items, &mut state);
        }
    }

    // Retained fields include external winit and Vello types. Dependency
    // changes require a new host even when local declarations did not change.
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    std::fs::read("../../Cargo.lock")
        .expect("read workspace Cargo.lock")
        .hash(&mut state);
    println!("cargo:rerun-if-changed=Cargo.toml");
    std::fs::read("Cargo.toml")
        .expect("read desktop2 Cargo.toml")
        .hash(&mut state);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    std::process::Command::new(rustc)
        .arg("--version")
        .arg("--verbose")
        .output()
        .expect("query rustc version")
        .stdout
        .hash(&mut state);

    println!(
        "cargo:rustc-env=JCODE_DESKTOP2_STATE_ABI={:016x}",
        state.finish()
    );
    println!(
        "cargo:rustc-env=JCODE_DESKTOP2_WORKER_BUILD={:016x}",
        build.finish()
    );
}

fn rust_files(dir: &Path, output: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn hash_state_items(items: &[syn::Item], state: &mut impl Hasher) {
    for item in items {
        match item {
            syn::Item::Struct(item) => item.to_token_stream().to_string().hash(state),
            syn::Item::Enum(item) => item.to_token_stream().to_string().hash(state),
            syn::Item::Union(item) => item.to_token_stream().to_string().hash(state),
            syn::Item::Type(item) => item.to_token_stream().to_string().hash(state),
            syn::Item::Mod(item) => {
                if let Some((_, items)) = &item.content {
                    hash_state_items(items, state);
                }
            }
            _ => {}
        }
    }
}
