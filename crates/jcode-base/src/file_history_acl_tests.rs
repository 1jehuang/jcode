use super::*;
use crate::file_history::tests::fixture;
#[cfg(target_os = "linux")]
use std::cell::Cell;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(target_os = "linux")]
use std::rc::Rc;

#[cfg(target_os = "linux")]
#[test]
fn linux_private_edit_and_rewind_settle_access_before_writing() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    for mode in [0o640, 0o4640] {
        let dir = fs::canonicalize(work.path())
            .unwrap()
            .join(format!("private-{mode:o}"));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("secret");
        fs::write(&path, b"private before").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let original = fs::metadata(&path).unwrap();
        let original_attrs = read_xattrs(&path).unwrap();
        assert!(!original_attrs.contains_key("system.posix_acl_access"));

        // Only new siblings inherit this named reader; the original excludes it.
        let mut acl = 2u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (1u16, 7u16, u32::MAX),
            (2, 4, unsafe { libc::geteuid() }.wrapping_add(1)),
            (4, 0, u32::MAX),
            (16, 4, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permissions.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        write_xattrs(
            &dir,
            &BTreeMap::from([("system.posix_acl_default".into(), acl)]),
        )
        .unwrap();
        let calls = Rc::new(Cell::new(0));
        let observed = Rc::clone(&calls);
        BEFORE_CONTENT_WRITE.with(|probe| {
            *probe.borrow_mut() = Some(Box::new(move |staged| {
                let metadata = fs::metadata(staged).unwrap();
                assert_eq!(
                    metadata.len(),
                    0,
                    "access must settle while the sibling is empty"
                );
                assert_eq!(metadata.permissions().mode() & 0o7777, mode);
                assert_eq!(
                    (metadata.uid(), metadata.gid()),
                    (original.uid(), original.gid())
                );
                assert_eq!(read_xattrs(staged).unwrap(), original_attrs);
                observed.set(observed.get() + 1);
            }));
        });
        let checkpoint = checkpoint("private-acl", 0, "edit private file").unwrap();
        mutate("private-acl", &path, Some(b"private after")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"private after");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            mode
        );
        restore("private-acl", &checkpoint.id).unwrap();
        BEFORE_CONTENT_WRITE.with(|probe| *probe.borrow_mut() = None);
        assert_eq!(calls.get(), 2, "both the edit and rewind must be observed");
        assert_eq!(fs::read(&path).unwrap(), b"private before");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
}

#[test]
fn denied_owner_restore_retains_permission_error_without_replacing_original() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path()).unwrap().join("shared");
    fs::write(&path, b"before").unwrap();
    let original = fs::metadata(&path).unwrap();
    let dir = directory("denied-owner").unwrap();
    let before = state(&dir, &path).unwrap();
    let mut after = before.clone();
    after.blob = Some(store_blob(&dir, b"after").unwrap());
    after.metadata.as_mut().unwrap().uid = 0;
    let error = apply(
        &dir,
        &Change {
            path: path.clone(),
            before,
            after,
        },
        false,
    )
    .unwrap_err();
    assert_eq!(
        error
            .root_cause()
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::PermissionDenied),
        "ownership failure must reach the ordinary-write fallback: {error:#}"
    );
    let retained = fs::metadata(&path).unwrap();
    assert_eq!(retained.ino(), original.ino());
    assert_eq!(
        (retained.uid(), retained.gid()),
        (original.uid(), original.gid())
    );
    assert_eq!(fs::read(&path).unwrap(), b"before");
    assert_eq!(fs::read_dir(work.path()).unwrap().count(), 1);
}

#[test]
fn denied_staging_edits_original_inode_and_blocks_rewind() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path()).unwrap().join("shared");
    fs::write(&path, b"before").unwrap();
    let original = fs::metadata(&path).unwrap();
    let checkpoint = checkpoint("denied-staging", 0, "shared edit").unwrap();
    let permissions = fs::metadata(work.path()).unwrap().permissions();
    fs::set_permissions(work.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = mutate("denied-staging", &path, Some(b"after"));
    fs::set_permissions(work.path(), permissions).unwrap();
    result.unwrap();
    let retained = fs::metadata(&path).unwrap();
    assert_eq!(retained.ino(), original.ino());
    assert_eq!(
        (retained.uid(), retained.gid()),
        (original.uid(), original.gid())
    );
    assert_eq!(retained.permissions().mode(), original.permissions().mode());
    assert_eq!(fs::read(&path).unwrap(), b"after");
    assert!(!checkpoints("denied-staging").unwrap()[0].can_restore_files);
    assert!(restore("denied-staging", &checkpoint.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"after");
}
