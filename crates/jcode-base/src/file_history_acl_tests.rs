use super::*;
use crate::file_history::tests::fixture;
use std::cell::Cell;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::rc::Rc;

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
