//! `jcode menubar` - a lightweight live indicator of how many jcode sessions
//! are running and how many are actively streaming a model response.
//!
//! On macOS this renders a native menu bar (`NSStatusItem`) item that updates
//! roughly once a second by reading the on-disk active-pid / streaming
//! registries (see `crate::session::session_counts`). On other platforms (and
//! with `--once` / `--json`) it just prints the current counts.

use anyhow::Result;
use serde::Serialize;

use crate::session::{self, SessionCounts, SessionPresence};

#[derive(Debug, Serialize)]
struct CountsReport {
    total: usize,
    streaming: usize,
}

impl From<SessionCounts> for CountsReport {
    fn from(counts: SessionCounts) -> Self {
        Self {
            total: counts.total,
            streaming: counts.streaming,
        }
    }
}

/// Return whether a persisted session represents a top-level session opened by
/// the user. The shared server also tracks internal debug/swarm runtimes in the
/// active-PID registry, but those are implementation details rather than menu
/// entries.
fn load_user_root_session(session_id: &str) -> Option<bool> {
    session::Session::load_startup_stub(session_id)
        .ok()
        .map(|session| session.parent_id.is_none() && !session.is_debug)
}

fn user_root_session_presence() -> Vec<SessionPresence> {
    session::user_session_presence()
        .into_iter()
        // A marker can briefly precede its first persisted snapshot. Keep an
        // unknown session visible for this read rather than hiding a new window.
        .filter(|presence| load_user_root_session(&presence.session_id).unwrap_or(true))
        .collect()
}

fn counts_for_presence(sessions: &[SessionPresence]) -> SessionCounts {
    SessionCounts {
        total: sessions.len(),
        streaming: sessions.iter().filter(|session| session.streaming).count(),
    }
}

/// Format the compact title shown next to the menu bar icon.
///
/// Always shows both the streaming and total counts directly in the menu bar
/// (e.g. "0/3" idle, "2/7" while streaming) so the live state is visible at a
/// glance without opening the dropdown. Icon-only when no sessions are running.
#[cfg(any(test, target_os = "macos"))]
pub(crate) fn format_menubar_title(counts: SessionCounts) -> String {
    if counts.total == 0 {
        String::new()
    } else {
        format!("{}/{}", counts.streaming, counts.total)
    }
}

/// Human-readable one-line summary used for `--once` and the menu header.
pub(crate) fn format_menubar_summary(counts: SessionCounts) -> String {
    format!(
        "{} streaming · {} session{} running",
        counts.streaming,
        counts.total,
        if counts.total == 1 { "" } else { "s" }
    )
}

/// Title for one session row in the dropdown menu. Match terminal window title
/// semantics: the emoji identifies the session, while a custom/todo-derived
/// display title explains the work. Do not repeat the generated animal name.
#[cfg(any(test, target_os = "macos"))]
pub(crate) fn format_session_menu_item_title(session_id: &str, streaming: bool) -> String {
    let display_title = crate::process_title::terminal_display_title_for_id(session_id);
    format_session_menu_item_title_with_display(session_id, display_title.as_deref(), streaming)
}

#[cfg(any(test, target_os = "macos"))]
fn format_session_menu_item_title_with_display(
    session_id: &str,
    display_title: Option<&str>,
    streaming: bool,
) -> String {
    let display = crate::id::extract_session_name(session_id).unwrap_or(session_id);
    let icon = crate::id::session_icon(display);
    let label = crate::process_title::terminal_window_title(icon, display_title, None, false);
    if streaming {
        format!("{label} · streaming")
    } else {
        label
    }
}

pub fn run_menubar_command(once: bool, json: bool) -> Result<()> {
    if json {
        let report = CountsReport::from(counts_for_presence(&user_root_session_presence()));
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }

    if once {
        println!(
            "{}",
            format_menubar_summary(counts_for_presence(&user_root_session_presence()))
        );
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        macos::run_status_item_app();
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!(
            "The live menu bar indicator is only available on macOS. \
             Showing current counts instead (use --once or --json for scripting):"
        );
        println!("{}", format_menubar_summary(session::user_session_counts()));
        Ok(())
    }
}

/// Ensure a single background `jcode menubar` helper is running on macOS so the
/// session-count indicator shows up automatically for every macOS user without
/// them needing to run `jcode menubar` by hand.
///
/// This is a best-effort, fire-and-forget singleton: it records the helper's
/// PID in the *global* `~/.jcode/menubar.pid` (see [`global_menubar_dir`]) and
/// only spawns a new detached process when no live helper is already running.
/// Failures are silently ignored so they never disrupt normal session startup.
///
/// The macOS menu bar is a single per-login-session resource, so this guards
/// hard against sandboxed jcode processes (tests, self-dev, onboarding) ever
/// spawning a helper: each such process runs with a throwaway `$JCODE_HOME`,
/// and without this guard every distinct sandbox home spawned its own helper
/// and drew its own duplicate status item into the one real menu bar.
#[cfg(target_os = "macos")]
pub fn ensure_menubar_helper_running() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // Allow users to opt out entirely.
    if std::env::var_os("JCODE_NO_MENUBAR").is_some() {
        return;
    }

    // Sandboxed jcode (tests / self-dev / onboarding, anything with a throwaway
    // `$JCODE_HOME`) must never manage the real user's global menu bar.
    if running_in_menubar_sandbox() {
        return;
    }

    let Some(dir) = global_menubar_dir() else {
        return;
    };
    let pid_path = dir.join("menubar.pid");

    // If a recorded helper PID is still alive, do nothing.
    if let Ok(raw) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = raw.trim().parse::<u32>() {
            if crate::platform::is_process_running(pid) {
                return;
            }
        }
    }

    let Ok(exe) = std::env::current_exe() else {
        return;
    };

    let mut command = Command::new(exe);
    command
        .arg("menubar")
        .env("JCODE_NO_MENUBAR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach from the parent's process group so the helper outlives this session.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    if let Ok(child) = command.spawn() {
        let _ = std::fs::write(&pid_path, child.id().to_string());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_menubar_helper_running() {}

/// Resolve the directory holding the *global* (per-OS-user) menu bar singleton
/// state - the "only one helper" lock and the helper pid file.
///
/// The macOS menu bar is a single per-login-session resource shared by every
/// jcode process for this user, so this state must live at a fixed location
/// that does **not** depend on `$JCODE_HOME`. Sandboxes (tests, self-dev,
/// onboarding) override `$JCODE_HOME` with throwaway temp dirs; anchoring to
/// the real home (`$HOME/.jcode`) gives every process the same lock inode so
/// the singleton actually holds across them. For a normal (non-sandboxed)
/// launch this is exactly `crate::storage::jcode_dir()`, so behavior for the
/// real user is unchanged.
#[cfg(target_os = "macos")]
fn global_menubar_dir() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".jcode");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// True when this process is a sandboxed jcode that must not own the real
/// user's global menu bar. A throwaway `$JCODE_HOME` (anything other than the
/// real `~/.jcode`) or an explicit test/temp marker means "sandbox".
#[cfg(target_os = "macos")]
fn running_in_menubar_sandbox() -> bool {
    is_menubar_sandbox(
        env_truthy("JCODE_TEST_SESSION"),
        env_truthy("JCODE_TEMP_SERVER"),
        std::env::var_os("JCODE_HOME").as_deref(),
        dirs::home_dir().map(|home| home.join(".jcode")).as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            let trimmed = value.trim();
            !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// Pure decision for [`running_in_menubar_sandbox`], split out so it can be
/// unit-tested without mutating process-global environment state.
///
/// - An explicit test/temp marker forces "sandbox".
/// - A `$JCODE_HOME` that differs from the real `~/.jcode` is a sandbox home.
/// - No override (or an override equal to the real home) is the real user.
#[cfg(target_os = "macos")]
fn is_menubar_sandbox(
    test_session: bool,
    temp_server: bool,
    custom_home: Option<&std::ffi::OsStr>,
    real_jcode_home: Option<&std::path::Path>,
) -> bool {
    if test_session || temp_server {
        return true;
    }

    // No explicit override: the real user's default `~/.jcode`.
    let Some(custom_home) = custom_home else {
        return false;
    };
    let custom = std::path::Path::new(custom_home);
    let Some(real) = real_jcode_home else {
        // No real home to compare against: treat any explicit override as a sandbox.
        return true;
    };
    let normalize =
        |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalize(custom) != normalize(real)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        counts_for_presence, format_menubar_summary, format_menubar_title,
        format_session_menu_item_title, load_user_root_session,
    };
    use crate::session::{self, SessionPresence};

    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::collections::{HashMap, HashSet};

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
        NSApplicationActivationPolicy, NSCellImagePosition, NSColor, NSFont, NSFontAttributeName,
        NSFontWeightRegular, NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem,
        NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    };
    use objc2_foundation::{
        NSAttributedString, NSDictionary, NSObject, NSString, NSUserDefaults, ns_string,
    };

    /// Poll interval for refreshing the counts (milliseconds).
    const REFRESH_INTERVAL_MS: u64 = 1000;

    /// A held singleton lock for the menu bar helper. Keeps the lock file open
    /// for the whole process lifetime; the kernel releases the advisory lock
    /// automatically when the process exits (including via `terminate:`).
    struct SingletonLock {
        #[allow(dead_code)]
        file: std::fs::File,
    }

    /// Acquire the exclusive, system-wide "only one menu bar helper" lock.
    ///
    /// Uses a non-blocking `flock(LOCK_EX | LOCK_NB)` on the *global*
    /// `~/.jcode/menubar.lock` (see [`super::global_menubar_dir`]) so the lock
    /// is shared across every jcode process for this OS user, including ones
    /// running with a sandboxed `$JCODE_HOME`. The menu bar itself is a single
    /// per-login-session resource, so the guard must be global too.
    ///
    /// Returns `Some(guard)` if we are the sole helper, or `None` if another
    /// live helper already holds the lock (in which case the caller should exit
    /// without creating a second status item). On any unexpected error we fall
    /// back to `Some` so a transient filesystem issue never permanently hides
    /// the indicator.
    fn acquire_singleton_lock() -> Option<SingletonLock> {
        use std::os::unix::io::AsRawFd;

        let dir = super::global_menubar_dir()?;
        let lock_path = dir.join("menubar.lock");
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => file,
            // If we cannot open the lock file at all, don't block the indicator.
            Err(_) => {
                return Some(SingletonLock {
                    file: dummy_file()?,
                });
            }
        };

        // SAFETY: `flock` on a valid fd. LOCK_NB makes this non-blocking.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Some(SingletonLock { file })
        } else {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                // Another helper holds the lock: we are the duplicate, bail out.
                Some(libc::EWOULDBLOCK) => None,
                // Unexpected error: don't permanently suppress the indicator.
                _ => Some(SingletonLock { file }),
            }
        }
    }

    /// Open `/dev/null` as a stand-in lock handle for the rare case where the
    /// real lock file cannot be created. Returns `None` only if even that
    /// fails, in which case the caller proceeds without a guard.
    fn dummy_file() -> Option<std::fs::File> {
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .ok()
    }

    /// Autosave name under which macOS persists the status item's position.
    const STATUS_ITEM_AUTOSAVE: &str = "jcode-menubar";

    /// Number of fixed items at the end of the menu (separator, New Window,
    /// separator, Quit). Session rows are inserted between the summary header
    /// and this tail.
    const MENU_TAIL_ITEMS: isize = 4;

    define_class!(
           // SAFETY: NSObject has no subclassing requirements and MenuHandler
           // does not implement Drop.
           #[unsafe(super(NSObject))]
           #[thread_kind = MainThreadOnly]
           #[name = "JcodeMenubarHandler"]
           struct MenuHandler;

           impl MenuHandler {
               /// Open the clicked session (stored in the item's representedObject)
               /// in a new terminal window via `jcode --resume <id>`.
               #[unsafe(method(openSession:))]
               fn open_session(&self, sender: &NSMenuItem)
    {
                   let Some(object) = sender.representedObject() else {
                       return;
                   };
                   let Ok(session_id) = object.downcast::<NSString>() else {
                       return;
                   };
                   launch_jcode_window(vec!["--resume".to_string(), session_id.to_string()]);
               }

               /// Launch a brand-new jcode session in a new terminal window.
               #[unsafe(method(newWindow:))]
               fn new_window(&self, _sender: &NSMenuItem) {
                   launch_jcode_window(Vec::new());
               }
           }
       );

    impl MenuHandler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Launch a jcode window off the main thread so slow terminal startup
    /// (osascript / `open`) never blocks the menu bar UI.
    fn launch_jcode_window(args: Vec<String>) {
        std::thread::spawn(move || {
            if let Err(err) = crate::setup_hints::launch_jcode_in_macos_terminal(&args) {
                crate::logging::warn(&format!(
                    "menubar: failed to launch jcode window ({args:?}): {err}"
                ));
            }
        });
    }

    pub(super) fn run_status_item_app() {
        let mtm = MainThreadMarker::new()
            .expect("jcode menubar must run on the main thread (the process entry point)");

        // Defense in depth: a process running under an explicit test/temp marker
        // must never paint into the real user's menu bar. The real protection
        // against duplicates is `ensure_menubar_helper_running` (which refuses
        // to spawn from sandboxes) plus the global singleton lock below, but a
        // stray `jcode menubar` invoked directly inside a test harness should
        // still never realize a status item.
        if super::env_truthy("JCODE_TEST_SESSION") || super::env_truthy("JCODE_TEMP_SERVER") {
            return;
        }

        // Enforce a single live menu bar helper. The pid-file fast path in
        // `ensure_menubar_helper_running` is best-effort and can race or be
        // bypassed entirely (e.g. a self-dev `target/.../jcode` and the
        // installed `~/.local/bin/jcode` both spawn helpers, or a reload
        // re-runs startup). Without a hard guard each extra helper creates its
        // own NSStatusItem, so the user ends up with a duplicate menu bar item
        // per spawn. Acquire an exclusive advisory lock here; if another helper
        // already holds it, exit immediately before creating any UI. The
        // returned guard must stay alive for the whole process lifetime (it is
        // held by `_singleton_lock` until `app.run()` is terminated).
        let Some(_singleton_lock) = acquire_singleton_lock() else {
            return;
        };

        let app = NSApplication::sharedApplication(mtm);
        // Accessory: no Dock icon, no main menu, just a menu bar item.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        // Follow the system's Light/Dark setting explicitly. `jcode` runs as a
        // bare Mach-O with no Info.plist app bundle, so AppKit defaults the
        // process to the light Aqua appearance and never auto-adopts macOS Dark
        // Mode. That made the status item's template icon and `labelColor` text
        // resolve to *black*, which is invisible on a dark menu bar (the exact
        // symptom: a black icon on an already-black bar). Pin the app's
        // appearance to match `AppleInterfaceStyle` so the template image and
        // dynamic colors render light on a dark bar (and dark on a light bar).
        sync_app_appearance(&app);

        let status_bar = NSStatusBar::systemStatusBar();
        let status_item: Retained<NSStatusItem> =
            status_bar.statusItemWithLength(NSVariableStatusItemLength);

        // Give the item a persistent identity and seed a sane preferred
        // position the first time. Without this, macOS appends brand-new
        // status items at the far left of the status area; if another app owns
        // an oversized status item (or the menu bar is crowded), a freshly
        // created item can be pushed completely off-screen and the user never
        // sees it. Seeding "NSStatusItem Preferred Position <name>" (distance
        // in points from the right screen edge) before the item is realized
        // places it among the system icons; afterwards macOS keeps tracking
        // the user's chosen position under the same key.
        unsafe {
            let defaults = NSUserDefaults::standardUserDefaults();
            let pos_key = NSString::from_str(&format!(
                "NSStatusItem Preferred Position {STATUS_ITEM_AUTOSAVE}"
            ));
            if defaults.objectForKey(&pos_key).is_none() {
                defaults.setInteger_forKey(550, &pos_key);
            }
            status_item.setAutosaveName(Some(&NSString::from_str(STATUS_ITEM_AUTOSAVE)));
        }

        // Style the button like a native menu bar extra: a template SF Symbol
        // (auto-adapts to light/dark menu bars and tinting) plus a compact
        // monospaced-digit count. Keeping the item narrow matters: macOS hides
        // wide status items first whenever the frontmost app's menus need the
        // space, which is why a verbose title appears and disappears depending
        // on which app is focused.
        let menu_bar_font_size = NSFont::menuBarFontOfSize(0.0).pointSize();
        let title_font =
            NSFont::monospacedDigitSystemFontOfSize_weight(menu_bar_font_size, unsafe {
                NSFontWeightRegular
            });
        if let Some(button) = status_item.button(mtm) {
            let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("terminal.fill"),
                Some(ns_string!("jcode sessions")),
            );
            if let Some(icon) = icon.as_deref() {
                icon.setTemplate(true);
                button.setImage(Some(icon));
                // Title on the left, icon on the right.
                button.setImagePosition(NSCellImagePosition::ImageTrailing);
            }
            button.setFont(Some(&title_font));
        }

        // The target of menu item actions. NSMenuItem holds its target weakly,
        // so keep a strong reference alive in the refresh closure below.
        let handler = MenuHandler::new(mtm);

        // Build the dropdown menu: summary header, dynamic session rows, then
        // a fixed tail (New Window / Quit).
        let menu = NSMenu::new(mtm);
        let summary_item = NSMenuItem::new(mtm);
        summary_item.setEnabled(false);
        menu.addItem(&summary_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let new_window_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("New jcode Window"),
                Some(sel!(newWindow:)),
                ns_string!("n"),
            )
        };
        unsafe { new_window_item.setTarget(Some(&handler)) };
        menu.addItem(&new_window_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Quit jcode menu bar"),
                Some(objc2::sel!(terminate:)),
                ns_string!("q"),
            )
        };
        menu.addItem(&quit_item);
        status_item.setMenu(Some(&menu));

        let last_sessions: RefCell<Vec<SessionPresence>> = RefCell::new(Vec::new());
        // Classification does not change for a session ID. Cache it so a large
        // population of internal workers costs one metadata read each rather
        // than one read per worker every second.
        let root_visibility: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
        let app_for_refresh = app.clone();
        let refresh = move || {
            // Re-sync the Light/Dark appearance each tick so toggling the
            // system theme at runtime keeps the status item visible (the
            // process won't auto-adopt the change on its own).
            sync_app_appearance(&app_for_refresh);

            let mut sessions = session::user_session_presence();
            let active_ids: HashSet<String> = sessions
                .iter()
                .map(|presence| presence.session_id.clone())
                .collect();
            root_visibility
                .borrow_mut()
                .retain(|session_id, _| active_ids.contains(session_id));
            sessions.retain(|presence| {
                if let Some(visible) = root_visibility.borrow().get(&presence.session_id).copied() {
                    return visible;
                }
                let Some(visible) = load_user_root_session(&presence.session_id) else {
                    // The marker may race the first session save. Retry next tick.
                    return true;
                };
                root_visibility
                    .borrow_mut()
                    .insert(presence.session_id.clone(), visible);
                visible
            });
            sessions.sort_by_key(|s| (Reverse(s.streaming), s.session_id.clone()));

            let counts = counts_for_presence(&sessions);
            if let Some(button) = status_item.button(mtm) {
                let title = format_menubar_title(counts);
                let attributed = attributed_title(&title, &title_font, counts.streaming > 0);
                button.setAttributedTitle(&attributed);
                // Tint the template icon to match: accent green while any
                // session is streaming, default (nil) otherwise so it follows
                // the menu bar's normal appearance.
                let tint: Option<Retained<NSColor>> = if counts.streaming > 0 {
                    Some(streaming_color())
                } else {
                    None
                };
                button.setContentTintColor(tint.as_deref());
            }
            summary_item.setTitle(&NSString::from_str(&format_menubar_summary(counts)));

            // Only touch the menu structure when the session set changed, so
            // an open menu is not visually disturbed every second.
            if *last_sessions.borrow() != sessions {
                rebuild_session_items(&menu, &handler, mtm, &sessions);
                *last_sessions.borrow_mut() = sessions;
            }
        };

        // Initial render before the run loop starts spinning.
        refresh();

        spawn_refresh_timer(refresh);

        // Run the Cocoa event loop. `terminate:` (the Quit item) exits the process.
        app.run();
    }

    /// Pin the application's appearance to the system Light/Dark setting.
    ///
    /// `jcode` runs as a bare executable without an `Info.plist` app bundle, so
    /// AppKit defaults the process to the light `Aqua` appearance and does not
    /// follow the user's macOS Dark Mode preference. With a light appearance the
    /// status item's template SF Symbol and `labelColor` title both resolve to a
    /// dark/black color, which is invisible against a dark menu bar. Reading
    /// `AppleInterfaceStyle` (absent => Light, "Dark" => Dark) and applying the
    /// matching named appearance makes those dynamic colors and template images
    /// render with proper contrast on whatever menu bar the user has.
    ///
    /// Safe to call repeatedly; setting the same appearance is a no-op.
    fn sync_app_appearance(app: &NSApplication) {
        let is_dark = {
            let defaults = NSUserDefaults::standardUserDefaults();
            defaults
                .stringForKey(ns_string!("AppleInterfaceStyle"))
                .map(|style| style.to_string().eq_ignore_ascii_case("dark"))
                .unwrap_or(false)
        };
        let name = if is_dark {
            unsafe { NSAppearanceNameDarkAqua }
        } else {
            unsafe { NSAppearanceNameAqua }
        };
        let appearance = NSAppearance::appearanceNamed(name);
        app.setAppearance(appearance.as_deref());
    }

    /// Color used for the count (and icon tint) while any session is actively
    /// streaming a response. A slightly muted system green that reads well in
    /// both light and dark menu bars.
    fn streaming_color() -> Retained<NSColor> {
        NSColor::systemGreenColor()
    }

    /// Build the colored menu bar title. While streaming, the count is drawn in
    /// the streaming color; when idle it uses the primary dynamic label color so
    /// it keeps full contrast against whatever the menu bar background is. (The
    /// previous secondary/"quiet" gray was nearly invisible on a black/dark menu
    /// bar.) `labelColor` is a dynamic system color, so AppKit resolves it at
    /// draw time using the status item button's effective appearance - white-ish
    /// on a dark menu bar, dark on a light one. The monospaced-digit font is
    /// applied so the width stays stable.
    fn attributed_title(
        title: &str,
        font: &NSFont,
        streaming: bool,
    ) -> Retained<NSAttributedString> {
        let string = NSString::from_str(title);
        let color = if streaming {
            streaming_color()
        } else {
            NSColor::labelColor()
        };
        let keys: [&NSString; 2] = [unsafe { NSForegroundColorAttributeName }, unsafe {
            NSFontAttributeName
        }];
        let color_obj: &AnyObject = &color;
        let font_obj: &AnyObject = font;
        let values: [&AnyObject; 2] = [color_obj, font_obj];
        let attrs = NSDictionary::from_slices(&keys, &values);
        unsafe { NSAttributedString::new_with_attributes(&string, &attrs) }
    }

    /// Replace the dynamic session rows between the summary header (index 0)
    /// and the fixed tail with one clickable row per running session.
    fn rebuild_session_items(
        menu: &NSMenu,
        handler: &MenuHandler,
        mtm: MainThreadMarker,
        sessions: &[SessionPresence],
    ) {
        while menu.numberOfItems() > 1 + MENU_TAIL_ITEMS {
            menu.removeItemAtIndex(1);
        }

        let mut index = 1;
        if !sessions.is_empty() {
            menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), index);
            index += 1;
        }
        for presence in sessions {
            let title = format_session_menu_item_title(&presence.session_id, presence.streaming);
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&title),
                    Some(sel!(openSession:)),
                    ns_string!(""),
                )
            };
            let target: &AnyObject = handler;
            unsafe {
                item.setTarget(Some(target));
                item.setRepresentedObject(Some(&NSString::from_str(&presence.session_id)));
            }
            item.setToolTip(Some(&NSString::from_str(&format!(
                "Open {} in a new terminal window",
                presence.session_id
            ))));
            menu.insertItem_atIndex(&item, index);
            index += 1;
        }
    }

    /// Schedule a repeating timer on the main run loop that re-renders the item.
    fn spawn_refresh_timer<F>(refresh: F)
    where
        F: Fn() + 'static,
    {
        use std::ptr::NonNull;

        use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};

        let interval = REFRESH_INTERVAL_MS as f64 / 1000.0;
        let block = block2::RcBlock::new(move |_timer: NonNull<NSTimer>| {
            refresh();
        });

        unsafe {
            let timer = NSTimer::timerWithTimeInterval_repeats_block(interval, true, &block);
            let run_loop = NSRunLoop::currentRunLoop();
            // Common modes (not just the default mode) so the counts and the
            // session list keep updating while the dropdown menu is open
            // (menu tracking runs the loop in NSEventTrackingRunLoopMode).
            run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes);
            // The run loop retains the timer; keep our reference alive too so the
            // owned closure (and its captured `status_item`) lives for the whole
            // process lifetime.
            std::mem::forget(timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionCounts;

    #[test]
    fn title_no_sessions_is_icon_only() {
        let title = format_menubar_title(SessionCounts {
            total: 0,
            streaming: 0,
        });
        assert_eq!(title, "");
    }

    #[test]
    fn title_idle_shows_zero_streaming_and_total() {
        let title = format_menubar_title(SessionCounts {
            total: 5,
            streaming: 0,
        });
        assert_eq!(title, "0/5");
    }

    #[test]
    fn title_streaming_shows_compact_ratio() {
        let title = format_menubar_title(SessionCounts {
            total: 7,
            streaming: 2,
        });
        assert_eq!(title, "2/7");
    }

    #[test]
    fn summary_pluralizes_sessions() {
        assert_eq!(
            format_menubar_summary(SessionCounts {
                total: 1,
                streaming: 0,
            }),
            "0 streaming · 1 session running"
        );
        assert_eq!(
            format_menubar_summary(SessionCounts {
                total: 3,
                streaming: 1,
            }),
            "1 streaming · 3 sessions running"
        );
    }

    #[test]
    fn session_menu_item_title_uses_meaningful_title_instead_of_animal_name() {
        let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
        assert_eq!(
            format_session_menu_item_title_with_display(
                session_id,
                Some("Improve menu labels"),
                false
            ),
            "🐃 Improve menu labels"
        );
        assert_eq!(
            format_session_menu_item_title_with_display(
                session_id,
                Some("Improve menu labels"),
                true
            ),
            "🐃 Improve menu labels · streaming"
        );
    }

    #[test]
    fn session_menu_item_title_is_icon_only_without_meaningful_title() {
        let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
        assert_eq!(
            format_session_menu_item_title_with_display(session_id, None, false),
            "🐃"
        );
        assert_eq!(
            format_session_menu_item_title_with_display("weird-id", None, false),
            "💫"
        );
    }

    #[test]
    fn session_menu_item_title_loads_persisted_todo_title() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::tempdir().expect("create temporary JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let session_id = "session_buffalo_1781229104969_6d487ff77287de4f";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("Generated session title".to_string()),
        );
        session.save().expect("save session");
        crate::todo::save_todos(
            session_id,
            &[crate::todo::TodoItem {
                content: "Use todo context in the menu bar".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                id: "menubar-label".to_string(),
                group: Some("Meaningful menu labels".to_string()),
                confidence: Some(95),
                completion_confidence: None,
                confidence_history: Vec::new(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert_eq!(
            format_session_menu_item_title(session_id, false),
            "🐃 Meaningful menu labels"
        );

        if let Some(previous_home) = previous_home {
            crate::env::set_var("JCODE_HOME", previous_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn menu_presence_includes_only_user_root_sessions() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::tempdir().expect("create temporary JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let mut root = crate::session::Session::create_with_id(
            "session_fox_root".to_string(),
            None,
            Some("Root work".to_string()),
        );
        root.mark_active();
        root.save().expect("save root session");

        let mut debug = crate::session::Session::create_with_id(
            "session_owl_debug".to_string(),
            None,
            Some("Internal worker".to_string()),
        );
        debug.set_debug(true);
        debug.mark_active();
        debug.save().expect("save debug session");

        let mut child = crate::session::Session::create_with_id(
            "session_bear_child".to_string(),
            Some(root.id.clone()),
            Some("Child work".to_string()),
        );
        child.mark_active();
        child.save().expect("save child session");

        let visible = user_root_session_presence();
        assert_eq!(
            visible
                .iter()
                .map(|presence| presence.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session_fox_root"]
        );
        assert_eq!(counts_for_presence(&visible).total, 1);

        root.mark_closed();
        debug.mark_closed();
        child.mark_closed();
        if let Some(previous_home) = previous_home {
            crate::env::set_var("JCODE_HOME", previous_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn counts_report_serializes_to_json() {
        let report = CountsReport::from(SessionCounts {
            total: 4,
            streaming: 2,
        });
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(json, r#"{"total":4,"streaming":2}"#);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn menubar_sandbox_detection() {
        use std::ffi::OsStr;
        use std::path::Path;

        let real = Path::new("/Users/me/.jcode");

        // Real user, no override: not a sandbox -> owns the menu bar.
        assert!(!is_menubar_sandbox(false, false, None, Some(real)));
        // Override equal to the real home is still the real user.
        assert!(!is_menubar_sandbox(
            false,
            false,
            Some(OsStr::new("/Users/me/.jcode")),
            Some(real),
        ));

        // Explicit test/temp markers force sandbox regardless of home.
        assert!(is_menubar_sandbox(true, false, None, Some(real)));
        assert!(is_menubar_sandbox(false, true, None, Some(real)));

        // A throwaway sandbox home (e2e / self-dev / onboarding) is a sandbox.
        assert!(is_menubar_sandbox(
            false,
            false,
            Some(OsStr::new("/private/tmp/jcode-e2e-home-xyz")),
            Some(real),
        ));

        // No discoverable real home: any explicit override is treated as sandbox.
        assert!(is_menubar_sandbox(
            false,
            false,
            Some(OsStr::new("/private/tmp/jcode-e2e-home-xyz")),
            None,
        ));
    }
}
