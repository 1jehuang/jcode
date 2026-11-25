use super::*;

/// Keep cloud environment values out of tool arguments and transcript output.
/// Read on each spawn so a just-completed opaque transfer is available to the
/// next command without changing the long-lived daemon's global environment.
pub(super) fn cloud_command_environment(
    working_dir: Option<&Path>,
) -> Result<std::collections::BTreeMap<String, String>> {
    let root = match working_dir {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()?,
    };
    crate::cloud_environment::load_environment(&root)
        .map_err(|_| anyhow::anyhow!("could not load saved cloud environment"))
}
