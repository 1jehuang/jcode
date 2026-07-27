//! Where this window is: the attached session's working directory.
//!
//! "Which directory am I typing into" is the one fact that decides whether a
//! reply is useful, and desktop2 previously never said it: the strip only
//! showed the leaf name of *other* sessions' directories, and a single-session
//! window showed nothing at all. The path is shown as a caption on the top row
//! and in the window title, so it is answerable both from inside the window and
//! from a compositor's window list.
//!
//! Pure functions over plain strings, so the formatting is tested without a
//! home directory or a window.

/// Path as a person writes it: `$HOME` collapsed to `~`, trailing slash
/// dropped.
///
/// Absolute paths under home are the common case and their first three
/// components are the same for every one of them, so spelling them out wastes
/// the width the interesting tail needs.
pub fn display_path(path: &str, home: Option<&str>) -> String {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() {
        return "/".to_string();
    }
    let Some(home) = home
        .map(|home| home.trim_end_matches('/'))
        .filter(|home| !home.is_empty())
    else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The current user's home directory, if the environment reports one.
pub fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|home| !home.is_empty())
}

/// Window title for a session in `working_dir`.
///
/// The directory leads: a taskbar or an alt-tab list truncates the tail, and
/// which checkout this window is for is what distinguishes two jcode windows
/// from each other.
pub fn window_title(working_dir: Option<&str>) -> String {
    match working_dir {
        Some(dir) => format!("{} - jcode", display_path(dir, home().as_deref())),
        None => "jcode desktop2".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_collapses_to_tilde() {
        assert_eq!(
            display_path("/home/j/jcode/crates/x", Some("/home/j")),
            "~/jcode/crates/x"
        );
        assert_eq!(display_path("/home/j", Some("/home/j")), "~");
        assert_eq!(display_path("/home/j/", Some("/home/j/")), "~");
    }

    /// A path that merely *starts with* the home string is not under it:
    /// `/home/jeremiah` must not become `~iah`.
    #[test]
    fn sibling_of_home_is_not_rewritten() {
        assert_eq!(
            display_path("/home/jeremiah/x", Some("/home/j")),
            "/home/jeremiah/x"
        );
    }

    #[test]
    fn paths_outside_home_are_left_alone() {
        assert_eq!(display_path("/srv/site", Some("/home/j")), "/srv/site");
        assert_eq!(display_path("/srv/site/", None), "/srv/site");
        assert_eq!(display_path("/", Some("/home/j")), "/");
    }

    #[test]
    fn title_names_the_directory_first() {
        // The formatting comes from display_path; this pins the shape only.
        let title = window_title(Some("/srv/site"));
        assert!(title.starts_with("/srv/site"), "{title}");
        assert!(title.ends_with("jcode"), "{title}");
        assert_eq!(window_title(None), "jcode desktop2");
    }
}
