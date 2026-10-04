use super::strip_deleted_suffix;
use std::path::PathBuf;

#[test]
fn strips_linux_deleted_marker() {
    let p = PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode (deleted)");
    assert_eq!(
        strip_deleted_suffix(p),
        PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode")
    );
}

#[test]
fn leaves_normal_paths_untouched() {
    let p = PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode");
    assert_eq!(strip_deleted_suffix(p.clone()), p);
}

#[test]
fn only_strips_trailing_marker() {
    // A path that merely contains the substring must not be altered.
    let p = PathBuf::from("/home/u/jcode (deleted)/jcode");
    assert_eq!(strip_deleted_suffix(p.clone()), p);
}
