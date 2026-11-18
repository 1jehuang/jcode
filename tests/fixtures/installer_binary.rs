//! Executable publication contract used by the isolated Windows installer tests.
//! No network, user PATH writes, daemon control, or production locking is simulated.
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["--version"] {
        println!(
            "jcode 0.0.1-eval ({})",
            if cfg!(fixture_v2) { "two" } else { "one" }
        );
        return Ok(());
    }
    if args.first().map(String::as_str) != Some("--no-update")
        || args.get(1).map(String::as_str) != Some("publish-build")
    {
        return Err(format!("unexpected fixture command: {args:?}").into());
    }
    let option = |key: &str| -> Result<&str, Box<dyn std::error::Error>> {
        let index = args
            .iter()
            .position(|arg| arg == key)
            .ok_or("missing option")?;
        Ok(args.get(index + 1).ok_or("missing value")?.as_str())
    };
    let source = PathBuf::from(option("--source")?).join("jcode.exe");
    let version = option("--version")?;
    let channel = option("--channel")?;
    if channel != "stable" {
        return Err("Windows installer must publish stable".into());
    }
    let builds =
        PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA")?).join("jcode/builds");
    let dest = builds.join("versions").join(version);
    fs::create_dir_all(&dest)?;
    fs::copy(&source, dest.join("jcode.exe"))?;
    let stable = builds.join("stable");
    fs::create_dir_all(&stable)?;
    fs::copy(&source, stable.join("jcode.exe"))?;
    fs::write(builds.join("stable-version"), format!("{version}\n"))?;
    let launcher = PathBuf::from(option("--launcher")?);
    fs::create_dir_all(launcher.parent().ok_or("launcher parent")?)?;
    fs::copy(&source, launcher)?;
    Ok(())
}
