use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let repo = manifest.join("../..");
    let docs_dir = repo.join("docs");
    println!(
        "cargo:rerun-if-changed={}",
        repo.join("README.md").display()
    );
    println!("cargo:rerun-if-changed={}", docs_dir.display());

    let mut files = vec![repo.join("README.md")];
    for entry in fs::read_dir(&docs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md")
            && entry.metadata()?.is_file()
        {
            files.push(path);
        }
    }
    files.sort();

    let mut generated = String::from("pub(crate) static JCODE_DOCS: &[(&str, &str)] = &[\n");
    for path in files {
        let relative = path.strip_prefix(&repo)?;
        let relative = slash_path(relative);
        generated.push_str(&format!(
            "    ({relative:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../{relative}\"))),\n"
        ));
    }
    generated.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR")?).join("jcode_docs.rs");
    match fs::read_to_string(&out) {
        Ok(previous) if previous == generated => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::write(out, generated)?;
    Ok(())
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
