//! Execute the rendered KDL's shell payload with a hermetic PATH. No compositor,
//! keyboard injector, or terminal can be reached, even on a Wayland desktop.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

struct RouterFixture {
    root: tempfile::TempDir,
}

impl RouterFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let fixture = Self { root };
        fixture.stub(
            "niri",
            r#"[ "$*" = 'msg -j focused-window' ] || exit 90
printf 'queried\n' >> "$QUERY_LOG"
[ "${FOCUS_FAILED:-0}" = 0 ] || exit 1
printf '%s\n' '{"app_id":"stub"}'"#,
        );
        fixture.stub(
            "jq",
            r#"[ "$#" = 2 ] && [ "$1" = '-r' ] && [ "$2" = '.app_id // empty' ] || exit 91
IFS= read -r input || exit 1
printf '%s\n' "$FOCUS_APP_ID""#,
        );
        fixture.stub(
            "wtype",
            r#"printf 'wtype\n'
printf '<%s>\n' "$@"
exit "${WTYPE_STATUS:-0}""#,
        );
        fixture.stub(
            "kitty",
            r#"printf 'kitty\n%s\n' "$PWD"
printf '<%s>\n' "$@""#,
        );
        fixture
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.root.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self, chord: &str, self_dev: bool, app_id: &str, extra: &[(&str, &str)]) -> Output {
        let dir = self.root.path().join("project's directory");
        std::fs::create_dir_all(&dir).unwrap();
        let hotkey = NiriHotkey {
            chord: KeyChord::parse(chord).unwrap(),
            dir: if extra.contains(&("MISSING_DIR", "1")) {
                self.root.path().join("missing").display().to_string()
            } else {
                dir.display().to_string()
            },
            label: "a quoted \"project\"".to_string(),
            self_dev,
        };
        let line = render_niri_bind_line(&hotkey, "/jcode's path/jcode", "kitty", "    ").unwrap();
        // The renderer's KDL string escapes are also JSON-compatible. Decode
        // the actual rendered payload rather than bypassing the KDL layer.
        let quoted = line.split_once("spawn \"sh\" \"-c\" ").unwrap().1;
        let shell: String = serde_json::from_str(quoted.strip_suffix("; }").unwrap()).unwrap();
        Command::new("/bin/sh")
            .args(["-c", &shell])
            .env_clear()
            .env("PATH", self.root.path())
            .env("HOME", self.root.path())
            .env("QUERY_LOG", self.root.path().join("queries"))
            .env("FOCUS_APP_ID", app_id)
            .envs(extra.iter().copied())
            .output()
            .unwrap()
    }

    fn queried(&self) -> bool {
        self.root.path().join("queries").exists()
    }
}

#[test]
fn regular_desktop_shortcuts_forward_exact_aliases() {
    for (chord, expected) in [
        (
            "cmd+;",
            "wtype\n<-M>\n<ctrl>\n<-M>\n<alt>\n<-k>\n<Return>\n<-m>\n<alt>\n<-m>\n<ctrl>\n",
        ),
        ("cmd+'", "wtype\n<-M>\n<ctrl>\n<-k>\n<t>\n<-m>\n<ctrl>\n"),
    ] {
        let fixture = RouterFixture::new();
        let output = fixture.run(chord, false, "jcode-desktop", &[]);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
        assert!(fixture.queried());
    }
}

#[test]
fn other_apps_and_missing_focus_preserve_launcher_arguments_and_directory() {
    for chord in ["cmd+;", "cmd+'"] {
        for app in [
            "firefox",
            "kitty",
            "",
            "org.jcode-desktop",
            "jcode-desktop-extra",
        ] {
            let fixture = RouterFixture::new();
            let output = fixture.run(chord, false, app, &[]);
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!(
                    "kitty\n{}/project's directory\n</jcode's path/jcode>\n<--spawn-hotkey>\n<{chord}>\n",
                    fixture.root.path().display()
                )
            );
        }
    }
}

#[test]
fn failed_focus_query_and_missing_directory_fall_back_to_home_launcher() {
    let fixture = RouterFixture::new();
    let output = fixture.run(
        "cmd+;",
        false,
        "jcode-desktop",
        &[("FOCUS_FAILED", "1"), ("MISSING_DIR", "1")],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "kitty\n{}\n</jcode's path/jcode>\n<--spawn-hotkey>\n<cmd+;>\n",
            fixture.root.path().display()
        )
    );
}

#[test]
fn self_dev_and_other_chords_do_not_even_query_focus() {
    for (chord, self_dev) in [
        ("cmd+;", true),
        ("cmd+'", true),
        ("cmd+shift+'", true),
        ("cmd+shift+;", false),
        ("cmd+shift+'", false),
        ("cmd+alt+;", false),
        ("cmd+ctrl+'", false),
        ("alt+;", false),
        ("cmd+a", false),
    ] {
        let fixture = RouterFixture::new();
        let output = fixture.run(chord, self_dev, "jcode-desktop", &[]);
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("kitty\n"), "{stdout}");
        assert_eq!(stdout.ends_with("<self-dev>\n"), self_dev);
        assert!(!fixture.queried(), "{chord}, self_dev={self_dev}");
    }
}

#[test]
fn failed_forwarding_does_not_launch_a_terminal() {
    let fixture = RouterFixture::new();
    let output = fixture.run("cmd+'", false, "jcode-desktop", &[("WTYPE_STATUS", "7")]);
    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("wtype\n"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("kitty"));
}
