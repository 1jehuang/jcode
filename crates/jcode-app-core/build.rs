use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo = manifest.join("../..");
    let docs_dir = repo.join("docs");
    println!(
        "cargo:rerun-if-changed={}",
        repo.join("README.md").display()
    );
    println!("cargo:rerun-if-changed={}", docs_dir.display());

    let mut files = vec![repo.join("README.md")];
    if let Ok(entries) = fs::read_dir(&docs_dir) {
        files.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        }));
    }
    files.sort();

    let mut generated = String::from("pub(crate) static JCODE_DOCS: &[(&str, &str)] = &[\n");
    for path in files {
        let relative = path
            .strip_prefix(&repo)
            .expect("documentation is in repository");
        let relative = slash_path(relative);
        generated.push_str(&format!(
            "    ({relative:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{relative}\"))),\n"
        ));
    }
    generated.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("jcode_docs.rs");
    fs::write(out, generated).expect("write generated Jcode documentation corpus");
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
