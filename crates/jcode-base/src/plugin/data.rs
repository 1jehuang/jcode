//! Persistent runtime data, separate from versioned plugin code.
//!
//! The Jcode home is a deliberate host mapping, not a claim about Claude's
//! configuration base. Short qualified IDs use Claude's PWt encoding.
//! A private ownership marker rejects its otherwise ambiguous encodings.
use super::PluginRoot;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const OWNER: &str = ".jcode-plugin-identity";

fn identity(plugin: &PluginRoot) -> Result<String> {
    // Match the manager's supported ordinary identities. npm/directory-source
    // IDs need their own explicit source model, never a cache-path inference.
    super::safe_name(&plugin.name)?;
    super::safe_name(&plugin.marketplace)?;
    Ok(format!("{}@{}", plugin.name, plugin.marketplace))
}

fn encoded_id(id: &str) -> String {
    let encoded: String = id
        .encode_utf16()
        .map(|c| match u8::try_from(c) {
            Ok(c) if c.is_ascii_alphanumeric() || b"-_".contains(&c) => c as char,
            _ => '-',
        })
        .collect();
    if encoded.len() <= 255 {
        encoded
    } else {
        // Dots cannot occur in the legacy encoding, keeping this namespace separate.
        format!("sha256.{}", hex::encode(Sha256::digest(id.as_bytes())))
    }
}

fn path_under(home: &Path, plugin: &PluginRoot) -> Result<PathBuf> {
    Ok(home
        .join("plugin-data")
        .join(encoded_id(&identity(plugin)?)))
}

fn data_home() -> Result<PathBuf> {
    let home = crate::storage::jcode_dir()?;
    ensure!(!home.as_os_str().is_empty(), "Jcode home must not be empty");
    Ok(if home.is_absolute() {
        home
    } else {
        std::env::current_dir()?.join(home)
    })
}

/// Compute a stable path without creating or claiming any directory.
/// Version, code location, workspace and install scope do not partition data.
pub fn data_path(plugin: &PluginRoot) -> Result<PathBuf> {
    path_under(&data_home()?, plugin)
}

/// Prepare data immediately before plugin execution, not during discovery.
/// Existing symlinks, non-directory path entries, and a different recorded
/// qualified identity are errors. Updates and uninstall never remove this tree.
pub fn prepare_data_dir(plugin: &PluginRoot) -> Result<PathBuf> {
    prepare_under(&data_home()?, plugin)
}

fn plain_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "plugin data directory is not a plain directory: {}",
        path.display()
    );
    Ok(())
}

fn prepare_under(home: &Path, plugin: &PluginRoot) -> Result<PathBuf> {
    let id = identity(plugin)?;
    let path = path_under(home, plugin)?;
    fs::create_dir_all(home).context("creating Jcode home for plugin data")?;
    plain_directory(home)?;
    let anchor = home.canonicalize()?;
    let mut current = home.to_path_buf();
    let encoded = encoded_id(&id);
    for part in ["plugin-data", encoded.as_str()] {
        current.push(part);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&current)
        };
        #[cfg(not(unix))]
        let created = fs::create_dir(&current);
        match created {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("creating plugin data directory"),
        }
        plain_directory(&current)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&current, fs::Permissions::from_mode(0o700))?;
        }
        ensure!(
            current.canonicalize()?.starts_with(&anchor),
            "plugin data directory escapes Jcode home"
        );
    }
    claim_identity(&path, &id)?;
    Ok(path)
}

fn claim_identity(path: &Path, id: &str) -> Result<()> {
    let marker = path.join(OWNER);
    let only_marker = || -> Result<bool> {
        for entry in fs::read_dir(path)? {
            if entry?.file_name() != OWNER {
                return Ok(false);
            }
        }
        Ok(true)
    };
    // Do not adopt populated directories with unknown ownership. A concurrent
    // claimant may have just created the marker, which is safe to arbitrate.
    if let Err(error) = fs::symlink_metadata(&marker) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
        ensure!(
            only_marker()? || fs::symlink_metadata(&marker).is_ok(),
            "plugin data directory has contents but no identity marker: {}",
            path.display()
        );
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.clone().create_new(true).open(&marker) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&marker)?;
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "plugin data identity marker is not a plain file"
            );
            options.open(&marker)?
        }
        Err(error) => return Err(error).context("claiming plugin data identity"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "plugin data identity marker is not a file"
    );
    // Serialize a plugin's concurrent hooks/MCP/LSP first launches. Each caller
    // reads after locking, including the process that won create_new.
    file.lock().context("locking plugin data identity")?;
    let mut owner = String::new();
    (&mut file).take(1024).read_to_string(&mut owner)?;
    if owner.is_empty() {
        ensure!(
            only_marker()?,
            "unclaimed plugin data directory is not empty"
        );
        file.write_all(id.as_bytes())?;
        file.sync_all()?;
    } else {
        ensure!(
            owner == id,
            "plugin data identity collision: {id} does not own {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginScope;

    fn plugin(root: &Path, name: &str, marketplace: &str) -> PluginRoot {
        PluginRoot {
            name: name.into(),
            marketplace: marketplace.into(),
            root: root.into(),
            version: Some("1".into()),
            scope: PluginScope::User,
        }
    }

    #[test]
    fn pure_path_is_qualified_stable_and_does_not_create_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("absent");
        let mut p = plugin(temp.path(), "Demo", "market");
        let expected = home.join("plugin-data/Demo-market");
        assert_eq!(path_under(&home, &p).unwrap(), expected);
        p.root = temp.path().join("different-code");
        p.version = Some("2".into());
        p.scope = PluginScope::Project;
        assert_eq!(path_under(&home, &p).unwrap(), expected);
        assert!(!home.exists());
        for invalid in ["", "../escape", "a@b"] {
            let invalid = plugin(temp.path(), invalid, "market");
            assert!(path_under(&home, &invalid).is_err());
            assert!(!home.exists());
        }
        p.marketplace = "other".into();
        assert_ne!(path_under(&home, &p).unwrap(), expected);
    }

    #[test]
    fn encoding_collision_refuses_cross_plugin_data_sharing() {
        let temp = tempfile::tempdir().unwrap();
        let a = plugin(temp.path(), "a-b", "c");
        let b = plugin(temp.path(), "a", "b-c");
        assert_eq!(
            path_under(temp.path(), &a).unwrap(),
            path_under(temp.path(), &b).unwrap()
        );
        let path = prepare_under(temp.path(), &a).unwrap();
        assert_eq!(prepare_under(temp.path(), &a).unwrap(), path);
        assert!(prepare_under(temp.path(), &b).is_err());
        assert_eq!(fs::read_to_string(path.join(OWNER)).unwrap(), "a-b@c");
    }

    #[test]
    fn concurrent_first_launches_claim_the_same_identity() {
        let temp = tempfile::tempdir().unwrap();
        let p = plugin(temp.path(), "demo", "market");
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        prepare_under(temp.path(), &p).unwrap()
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap(), path_under(temp.path(), &p).unwrap());
            }
        });
    }

    #[test]
    fn distinct_qualified_identities_have_independent_state() {
        let temp = tempfile::tempdir().unwrap();
        let a = prepare_under(temp.path(), &plugin(temp.path(), "demo", "first")).unwrap();
        let b = prepare_under(temp.path(), &plugin(temp.path(), "demo", "second")).unwrap();
        assert_ne!(a, b);
        fs::write(a.join("state"), "first").unwrap();
        assert!(!b.join("state").exists());
    }

    #[test]
    fn listing_is_inert_and_update_uninstall_preserve_data() {
        let (_temp, manager, market) = crate::plugin::tests::fixture();
        manager.add_marketplace(market.to_str().unwrap()).unwrap();
        let installed = manager.install("demo", PluginScope::User).unwrap();
        let root = manager.enabled_roots().unwrap().remove(0);
        let path = path_under(&manager.user_root, &root).unwrap();
        assert!(!path.exists());
        assert!(!manager.list().unwrap().is_empty());
        assert!(!path.exists());
        assert_eq!(prepare_under(&manager.user_root, &root).unwrap(), path);
        fs::write(path.join("state"), "keep me").unwrap();
        fs::write(
            market.join("demo/.claude-plugin/plugin.json"),
            r#"{"name":"demo","version":"2.0.0"}"#,
        )
        .unwrap();
        manager.update_marketplace("fixture").unwrap();
        let updated = manager.update("demo", PluginScope::User).unwrap();
        assert_ne!(updated.root, installed.root);
        assert_eq!(
            path_under(
                &manager.user_root,
                &manager.enabled_roots().unwrap().remove(0)
            )
            .unwrap(),
            path
        );
        manager.uninstall("demo", PluginScope::User).unwrap();
        assert_eq!(fs::read_to_string(path.join("state")).unwrap(), "keep me");
    }

    #[test]
    fn existing_files_and_unowned_contents_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let p = plugin(temp.path(), "demo", "market");
        fs::write(temp.path().join("plugin-data"), "file").unwrap();
        assert!(prepare_under(temp.path(), &p).is_err());
        fs::remove_file(temp.path().join("plugin-data")).unwrap();
        let path = path_under(temp.path(), &p).unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("unowned"), "state").unwrap();
        assert!(prepare_under(temp.path(), &p).is_err());
        assert!(!path.join(OWNER).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_data_ancestors_leaf_and_marker_are_refused() {
        use std::os::unix::fs::symlink;
        for relative in ["plugin-data", "plugin-data/demo-market"] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let outside = temp.path().join("outside");
            fs::create_dir_all(&outside).unwrap();
            let link = home.join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside, &link).unwrap();
            assert!(prepare_under(&home, &plugin(temp.path(), "demo", "market")).is_err());
            assert!(fs::read_dir(outside).unwrap().next().is_none());
        }
        let temp = tempfile::tempdir().unwrap();
        let p = plugin(temp.path(), "demo", "market");
        let path = path_under(temp.path(), &p).unwrap();
        fs::create_dir_all(&path).unwrap();
        let outside = temp.path().join("outside-marker");
        fs::write(&outside, "demo@market").unwrap();
        symlink(&outside, path.join(OWNER)).unwrap();
        assert!(prepare_under(temp.path(), &p).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn data_directories_are_private_before_state_is_written() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let p = plugin(temp.path(), "demo", "market");
        let base = temp.path().join("plugin-data");
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
        let path = prepare_under(temp.path(), &p).unwrap();
        for dir in [&base, &path] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let extension = temp.path().join("plugins/data");
        assert!(!extension.exists());
    }
    #[test]
    fn maximum_catalog_identities_prepare_bounded_data_paths() {
        let (_temp, manager, market) = crate::plugin::tests::fixture();
        let name = "n".repeat(128);
        let marketplace = "m".repeat(128);
        fs::write(
            market.join(".claude-plugin/marketplace.json"),
            serde_json::json!({
                "name":marketplace, "plugins":[{"name":name,"source":"./demo"}]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            market.join("demo/.claude-plugin/plugin.json"),
            serde_json::json!({
                "name":name, "version":"1"
            })
            .to_string(),
        )
        .unwrap();
        manager.add_marketplace(market.to_str().unwrap()).unwrap();
        manager
            .install(&format!("{name}@{marketplace}"), PluginScope::User)
            .unwrap();
        let mut plugin = manager.enabled_roots().unwrap().remove(0);
        let path = prepare_under(&manager.user_root, &plugin).unwrap();
        assert!(path.file_name().unwrap().to_str().unwrap().len() <= 255);
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("sha256.")
        );
        assert_eq!(
            fs::read_to_string(path.join(OWNER)).unwrap(),
            format!("{name}@{marketplace}")
        );
        assert_eq!(prepare_under(&manager.user_root, &plugin).unwrap(), path);
        plugin.marketplace = "m".repeat(126);
        let legacy = format!("{}-{}", plugin.name, plugin.marketplace);
        assert_eq!(legacy.len(), 255);
        let boundary = prepare_under(&manager.user_root, &plugin).unwrap();
        assert_eq!(boundary.file_name().unwrap().to_str().unwrap(), legacy);
    }

    #[test]
    fn long_legacy_encoding_collisions_keep_independent_qualified_owners() {
        let temp = tempfile::tempdir().unwrap();
        let a = plugin(
            temp.path(),
            &"a".repeat(127),
            &format!("-{}", "b".repeat(127)),
        );
        let b = plugin(
            temp.path(),
            &format!("{}-", "a".repeat(127)),
            &"b".repeat(127),
        );
        let a_id = identity(&a).unwrap();
        let b_id = identity(&b).unwrap();
        assert_eq!(a_id.replace('@', "-"), b_id.replace('@', "-"));
        assert_eq!(a_id.len(), 256);
        let a_path = prepare_under(temp.path(), &a).unwrap();
        fs::write(a_path.join("state"), "only A").unwrap();
        let b_path = prepare_under(temp.path(), &b).unwrap();
        assert_ne!(a_path, b_path);
        assert!(!b_path.join("state").exists());
        assert_eq!(fs::read_to_string(a_path.join(OWNER)).unwrap(), a_id);
        assert_eq!(fs::read_to_string(b_path.join(OWNER)).unwrap(), b_id);
        fs::write(a_path.join(OWNER), &b_id).unwrap();
        assert!(prepare_under(temp.path(), &a).is_err());
        assert_eq!(fs::read_to_string(a_path.join("state")).unwrap(), "only A");
    }
}
