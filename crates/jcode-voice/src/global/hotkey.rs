//! Linux-only, polling global Copilot hold listener. One Listener per workspace.
//! No threads, grabs, key logging, or ordinary-key reads. The caller must keep
//! polling (25–50 ms), including while unfocused, set_eligible for chat/modal
//! eligibility, and call set_active on focus
//! changes. A newly focused workspace wins the next press, not an existing hold.
//! Linux always delivers EV_SYN framing despite masks. SYN_REPORT is ignored,
//! SYN_DROPPED fails closed. No ordinary EV_KEY events are read.
//! `new(&[PathBuf])` accepts an explicit event-device allowlist and registers only. The first `poll` discovers/opens eligible devices.
#![cfg(target_os = "linux")]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const F23: usize = 193;
const ASSISTANT: usize = 0x247;
const KEYS: [usize; 10] = [F23, ASSISTANT, 42, 54, 125, 126, 29, 97, 56, 100];
const TTL: u64 = 3_000_000_000;
const RETRY: Duration = Duration::from_secs(2);
const DEADLINE: Duration = Duration::from_secs(120);
const KEY_BYTES: usize = 96;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Press,
    Release,
    /// Abort without finalizing a transcript. Safety/failure, not physical key-up.
    Cancel,
    /// Shift+Copilot: open a new single-panel window that records this hold.
    /// Its physical key-up still arrives here as Release.
    Spawn,
    /// A spawned window opened after its Shift+Copilot hold already ended.
    /// Start a click-style recording instead of losing the request.
    Tap,
}

/// How long a freshly spawned window waits to adopt the hold that opened it.
const ADOPT_WINDOW: Duration = Duration::from_secs(10);

// Linux input_event, including the native ABI's actual timeval layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}
#[repr(C)]
struct InputMask {
    kind: u32,
    size: u32,
    pointer: u64,
}
fn request(write: bool, nr: u8, size: usize) -> libc::c_ulong {
    (((if write { 1u32 } else { 2 }) << 30)
        | ((size as u32) << 16)
        | (u32::from(b'E') << 8)
        | u32::from(nr)) as libc::c_ulong
}
fn ioctl<T>(fd: &File, req: libc::c_ulong, value: &mut T) -> io::Result<()> {
    // SAFETY: each caller supplies the exact UAPI layout and request size.
    if unsafe { libc::ioctl(fd.as_raw_fd(), req, value as *mut T) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn bit(bits: &[u8], key: usize) -> bool {
    bits[key / 8] & (1 << (key % 8)) != 0
}
fn mask(file: &File, kind: u32, bits: &[u8]) -> io::Result<()> {
    let mut value = InputMask {
        kind,
        size: bits.len() as u32,
        pointer: bits.as_ptr() as u64,
    };
    ioctl(
        file,
        request(true, 0x93, size_of::<InputMask>()),
        &mut value,
    )
}
fn now() -> io::Result<u64> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid writable timespec, BOOTTIME is shared across processes.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
}
fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}
struct Lock(File);
impl Lock {
    fn take(path: &Path) -> io::Result<Self> {
        let f = private_file(path)?;
        // SAFETY: valid fd. Never block the UI thread.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(f))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: owned valid fd. The persistent lock inode must never be unlinked.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

struct Registration {
    dir: PathBuf,
    path: PathBuf,
    file: File,
    focus: u64,
    active: bool,
    published: Option<(u64, bool, u64)>,
}
impl Registration {
    fn new(dir: PathBuf) -> io::Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = fs::symlink_metadata(&dir)?;
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe voice runtime directory",
            ));
        }
        let id = format!(
            "reg-{}-{}-{}",
            std::process::id(),
            now()?,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = dir.join(id);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        Ok(Self {
            dir,
            path,
            file,
            focus: 0,
            active: false,
            published: None,
        })
    }
    fn update(&mut self, active: Option<bool>, eligible: bool) -> io::Result<()> {
        let stamp = now()?;
        if let Some(active) = active {
            if active && !self.active {
                self.focus = stamp;
            }
            self.active = active;
        }
        if self
            .published
            .is_some_and(|(focus, was_eligible, written)| {
                focus == self.focus
                    && was_eligible == eligible
                    && stamp.saturating_sub(written) < 500_000_000
            })
        {
            return Ok(());
        }
        let _guard = match Lock::take(&self.dir.join("routing.lock")) {
            Ok(guard) => guard,
            // Another host is routing a press. Preserve a live hold and retry
            // the heartbeat next poll rather than releasing on contention.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        write!(
            self.file,
            "{} {} {}\n",
            self.focus,
            stamp,
            u8::from(eligible)
        )?;
        self.published = Some((self.focus, eligible, stamp));
        Ok(())
    }
    fn capture(&self, event_stamp: u64) -> io::Result<Option<Lock>> {
        let _guard = match Lock::take(&self.dir.join("routing.lock")) {
            Ok(guard) => guard,
            Err(e) => return Err(e),
        };
        let stamp = now()?;
        let mut winner = None;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with("reg-") {
                continue;
            }
            let mut text = String::new();
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(entry.path());
            let Ok(file) = file else { continue };
            if file.take(128).read_to_string(&mut text).is_err() {
                continue;
            }
            let fields: Vec<_> = text
                .split_whitespace()
                .filter_map(|v| v.parse::<u64>().ok())
                .collect();
            if let [focus, heartbeat, 1] = fields.as_slice() {
                if *focus > 0 && *heartbeat <= stamp && stamp - heartbeat <= TTL {
                    let candidate = (*focus, entry.path());
                    if winner.as_ref().is_none_or(|w| candidate > *w) {
                        winner = Some(candidate);
                    }
                }
            }
        }
        if winner.is_some_and(|(_, path)| path == self.path) {
            match Lock::take(&self.dir.join("capture.lock")) {
                Ok(mut lock) => {
                    let mut previous = String::new();
                    (&lock.0).take(64).read_to_string(&mut previous)?;
                    if previous
                        .trim()
                        .parse::<u64>()
                        .is_ok_and(|p| event_stamp <= p)
                    {
                        return Ok(None);
                    }
                    lock.0.seek(SeekFrom::Start(0))?;
                    lock.0.set_len(0)?;
                    writeln!(lock.0, "{event_stamp}")?;
                    Ok(Some(lock))
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(e),
            }
        } else {
            Ok(None)
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Default)]
struct Decoder {
    down: BTreeSet<usize>,
    suppressed: BTreeSet<usize>,
    trigger: Option<usize>,
    /// Press order of the keys in `down`. Keys already held at open are 0.
    order: BTreeMap<usize, u64>,
    presses: u64,
}
impl Decoder {
    fn snapshot(bits: &[u8; KEY_BYTES]) -> Self {
        let down: BTreeSet<_> = KEYS.into_iter().filter(|k| bit(bits, *k)).collect();
        let suppressed = down
            .iter()
            .copied()
            .filter(|k| *k == F23 || *k == ASSISTANT)
            .collect();
        Self {
            down,
            suppressed,
            trigger: None,
            order: BTreeMap::new(),
            presses: 0,
        }
    }
    /// Replace key state with the kernel's current view after dropped events.
    /// Returns whether the trigger key is still held.
    fn resync(&mut self, held: &[u8; KEY_BYTES]) -> bool {
        self.down = KEYS.into_iter().filter(|k| bit(held, *k)).collect();
        let down = &self.down;
        self.order.retain(|key, _| down.contains(key));
        self.suppressed.retain(|key| down.contains(key));
        if self.trigger.is_some_and(|key| !self.down.contains(&key)) {
            self.trigger = None;
        }
        self.trigger.is_some()
    }
    fn pressed_at(&self, key: usize) -> Option<u64> {
        self.down
            .contains(&key)
            .then(|| self.order.get(&key).copied().unwrap_or(0))
    }
    /// The Copilot key synthesizes Meta, then Shift, then F23. A Shift the
    /// user was already holding therefore went down before that Meta (the
    /// kernel drops the synthetic duplicate). Right Shift is never synthetic.
    fn user_shift(&self) -> bool {
        if self.down.contains(&54) {
            return true;
        }
        let Some(shift) = self.pressed_at(42) else {
            return false;
        };
        [125, 126]
            .iter()
            .filter_map(|key| self.pressed_at(*key))
            .min()
            .is_none_or(|meta| shift < meta)
    }
    fn event(&mut self, kind: u16, code: u16, value: i32) -> Option<Edge> {
        let key = usize::from(code);
        if kind != 1 || !KEYS.contains(&key) || !(0..=2).contains(&value) {
            return None;
        }
        if value == 2 {
            return None;
        }
        if value == 0 {
            self.down.remove(&key);
            self.order.remove(&key);
            self.suppressed.remove(&key);
            if self.trigger == Some(key) {
                self.trigger = None;
                return Some(Edge::Release);
            }
            return None;
        }
        if !self.down.insert(key) {
            return None;
        }
        self.presses += 1;
        self.order.insert(key, self.presses);
        if self.suppressed.contains(&key) || self.trigger.is_some() {
            return None;
        }
        let shift = self.down.contains(&42) || self.down.contains(&54);
        let meta = self.down.contains(&125) || self.down.contains(&126);
        let blocked = [29, 97, 56, 100].iter().any(|key| self.down.contains(key));
        if blocked {
            return None;
        }
        let edge = match key {
            // Bare Assistant, or its Super+Shift form, is the normal hold.
            ASSISTANT if shift == meta => Edge::Press,
            F23 if shift && meta && self.user_shift() => Edge::Spawn,
            F23 if shift && meta => Edge::Press,
            _ => return None,
        };
        self.trigger = Some(key);
        Some(edge)
    }
}
// sysfs prints native unsigned-long bitmap words, most significant first.
fn advertised(text: &str, key: usize) -> io::Result<bool> {
    let words = text
        .split_whitespace()
        .rev()
        .map(|s| usize::from_str_radix(s, 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid capability bitmap"))?;
    Ok(words
        .get(key / usize::BITS as usize)
        .is_some_and(|w| w & (1usize << (key % usize::BITS as usize)) != 0))
}

struct Device {
    file: File,
    decoder: Decoder,
}
impl Device {
    fn open(path: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        if !file.metadata()?.file_type().is_char_device() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "keyboard must be a character device",
            ));
        }
        let mut keys = [0u8; KEY_BYTES];
        ioctl(&file, request(false, 0x21, KEY_BYTES), &mut keys)?; // EVIOCGBIT(EV_KEY)
        if !bit(&keys, F23) && !bit(&keys, ASSISTANT) {
            return Ok(None);
        }
        let mut selected = [0u8; KEY_BYTES];
        for key in KEYS {
            selected[key / 8] |= 1 << (key % 8);
        }
        // Set all per-type masks, including the event-type mask (type zero).
        // EV_SYN is special in evdev. Only SYN_DROPPED is acted upon.
        mask(&file, 1, &selected)?;
        for kind in 2..=0x1f {
            mask(&file, kind, &[0; KEY_BYTES])?;
        }
        mask(&file, 0, &[0b10, 0, 0, 0, 0, 0, 0, 0])?;
        // evdev starts on REALTIME. Changing clocks discards any events queued
        // between open and mask installation, before our first read.
        let mut clock: libc::c_int = libc::CLOCK_REALTIME;
        ioctl(
            &file,
            request(true, 0xa0, size_of::<libc::c_int>()),
            &mut clock,
        )?;
        clock = libc::CLOCK_MONOTONIC;
        ioctl(
            &file,
            request(true, 0xa0, size_of::<libc::c_int>()),
            &mut clock,
        )?;
        let mut held = [0u8; KEY_BYTES];
        ioctl(&file, request(false, 0x18, KEY_BYTES), &mut held)?;
        Ok(Some(Self {
            file,
            decoder: Decoder::snapshot(&held),
        }))
    }
    /// After SYN_DROPPED, discard the stale queue and re-read which keys are
    /// down (EVIOCGKEY). Returns whether the trigger key is still held.
    fn resync(&mut self) -> io::Result<bool> {
        while let Some(event) = self.read_raw()? {
            if event.kind == 0 && event.code == 0 {
                break;
            }
        }
        let mut held = [0u8; KEY_BYTES];
        ioctl(&self.file, request(false, 0x18, KEY_BYTES), &mut held)?;
        Ok(self.decoder.resync(&held))
    }
    fn read(&mut self) -> io::Result<Option<InputEvent>> {
        self.read_raw()
    }
    fn read_raw(&mut self) -> io::Result<Option<InputEvent>> {
        let mut event = InputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            kind: 0,
            code: 0,
            value: 0,
        };
        // SAFETY: writable, aligned input_event with precisely the native size.
        let n = unsafe {
            libc::read(
                self.file.as_raw_fd(),
                (&mut event as *mut InputEvent).cast(),
                size_of::<InputEvent>(),
            )
        };
        if n == size_of::<InputEvent>() as isize {
            Ok(Some(event))
        } else if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(e)
            }
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "input disconnected",
            ))
        }
    }
}

fn event_node(path: &Path) -> bool {
    path.parent() == Some(Path::new("/dev/input"))
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("event"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn allowed_device_path(path: &Path) -> bool {
    event_node(path)
        || (matches!(
            path.parent().and_then(Path::to_str),
            Some("/dev/input/by-path" | "/dev/input/by-id")
        ) && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-event-kbd")))
}

pub struct Listener {
    registration: Registration,
    allowlist: Vec<PathBuf>,
    eligible: bool,
    devices: BTreeMap<PathBuf, Device>,
    /// A spawned window adopts its opener's hold without the routing lock,
    /// which the opening window still holds until key-up.
    capture: Option<(PathBuf, Instant, Option<Lock>)>,
    pending: Option<(PathBuf, u64, Edge)>,
    adopt_until: Option<Instant>,
    next_scan: Instant,
    healthy: bool,
    registration_ok: bool,
}
impl Listener {
    pub fn new(allowlist: &[PathBuf]) -> io::Result<Self> {
        for path in allowlist {
            if !allowed_device_path(path) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "expected an explicit evdev event node or stable by-path/by-id keyboard path",
                ));
            }
        }
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR missing"))?;
        let runtime = PathBuf::from(runtime);
        if !runtime.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime path must be absolute",
            ));
        }
        let meta = fs::symlink_metadata(&runtime)?;
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe XDG_RUNTIME_DIR",
            ));
        }
        Ok(Self {
            registration: Registration::new(runtime.join("jcode-global-voice-v1"))?,
            allowlist: allowlist.to_vec(),
            eligible: false,
            devices: BTreeMap::new(),
            capture: None,
            pending: None,
            adopt_until: None,
            next_scan: Instant::now(),
            healthy: false,
            registration_ok: false,
        })
    }
    pub fn set_active(&mut self, active: bool) {
        self.registration_ok = self
            .registration
            .update(
                Some(active),
                self.eligible && self.healthy && !self.devices.is_empty(),
            )
            .is_ok();
    }
    /// Call for a valid chat with no modal. Defaults false (explicit opt-in).
    /// Disabling eligibility cancels any hold on the next poll.
    pub fn set_eligible(&mut self, eligible: bool) {
        self.eligible = eligible;
        self.registration_ok = self
            .registration
            .update(
                None,
                self.eligible && self.healthy && !self.devices.is_empty(),
            )
            .is_ok();
    }
    /// Treat a Copilot key that is already down as this window's own hold.
    /// Used once by a window that Shift+Copilot just opened.
    pub fn adopt_held_key(&mut self) {
        self.adopt_until = Some(Instant::now() + ADOPT_WINDOW);
    }
    fn adopt(&mut self, edges: &mut Vec<Edge>) {
        let Some(until) = self.adopt_until else {
            return;
        };
        if Instant::now() >= until {
            self.adopt_until = None;
            return;
        }
        if self.capture.is_some() || !self.ready() {
            return;
        }
        self.adopt_until = None;
        let held = self.devices.iter_mut().find_map(|(path, device)| {
            let key = [F23, ASSISTANT]
                .into_iter()
                .find(|key| device.decoder.down.contains(key))?;
            device.decoder.suppressed.remove(&key);
            device.decoder.trigger = Some(key);
            Some(path.clone())
        });
        match held {
            Some(path) => {
                self.capture = Some((path, Instant::now(), None));
                edges.push(Edge::Press);
            }
            None => edges.push(Edge::Tap),
        }
    }
    pub fn ready(&self) -> bool {
        self.eligible && self.healthy && self.registration_ok && !self.devices.is_empty()
    }
    fn release(&mut self, edges: &mut Vec<Edge>) {
        if self.capture.take().is_some() {
            edges.push(Edge::Release);
        }
    }
    fn cancel(&mut self, reason: &str, edges: &mut Vec<Edge>) {
        self.pending = None;
        if let Some((_, start, _)) = self.capture.take() {
            eprintln!(
                "global voice: listener cancel after {:.1}s: {reason}",
                start.elapsed().as_secs_f32()
            );
            edges.push(Edge::Cancel);
        }
    }
    fn scan(&mut self) -> io::Result<()> {
        let mut paths = BTreeSet::new();
        for path in &self.allowlist {
            if !path.try_exists()? {
                continue;
            }
            let path = fs::canonicalize(path)?;
            if !event_node(&path) || !fs::metadata(&path)?.file_type().is_char_device() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "allowlisted keyboard resolves outside evdev",
                ));
            }
            // Only explicitly opted-in event nodes are inspected. Capability
            // discovery still occurs before opening the allowed evdev device.
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing event name"))?;
            let caps = fs::read_to_string(
                Path::new("/sys/class/input")
                    .join(name)
                    .join("device/capabilities/key"),
            )?;
            if advertised(&caps, F23)? || advertised(&caps, ASSISTANT)? {
                paths.insert(path);
            }
        }
        self.devices.retain(|path, _| paths.contains(path));
        for path in paths {
            if !self.devices.contains_key(&path) {
                if let Some(device) = Device::open(&path)? {
                    self.devices.insert(path, device);
                }
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<Edge> {
        let mut edges = Vec::new();
        if Instant::now() >= self.next_scan {
            self.next_scan = Instant::now() + RETRY;
            self.healthy = self.scan().is_ok();
            if !self.healthy {
                self.devices.clear();
            }
        }
        self.registration_ok = self
            .registration
            .update(
                None,
                self.eligible && self.healthy && !self.devices.is_empty(),
            )
            .is_ok();
        if self.capture.is_some() {
            let reason = if !self.eligible {
                Some("no eligible chat (voice target lost or sign-in modal)")
            } else if !self.healthy {
                Some("keyboard rescan failed")
            } else if self.devices.is_empty() {
                Some("no keyboard devices")
            } else if !self.registration_ok {
                Some("routing registration failed")
            } else if let Some((p, start, _)) = self.capture.as_ref() {
                if !self.devices.contains_key(p) {
                    Some("capturing keyboard disappeared")
                } else if start.elapsed() >= DEADLINE {
                    Some("hold exceeded the 120s deadline")
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(reason) = reason {
                self.cancel(reason, &mut edges);
            }
        } else if !self.ready() {
            self.cancel("not ready", &mut edges);
        }
        let paths: Vec<_> = self.devices.keys().cloned().collect();
        for path in paths {
            // Bound UI work even under an endlessly repeating device.
            for _ in 0..256 {
                let device = self.devices.get_mut(&path).expect("known device");
                let event = match device.read() {
                    // SYN_DROPPED: the poll stalled (build load, reload) and
                    // key-repeat overflowed the buffer. Resync, never cancel.
                    Ok(Some(event)) if event.kind == 0 && event.code == 3 => {
                        let capturing = self.capture.as_ref().is_some_and(|(p, _, _)| p == &path);
                        match device.resync() {
                            Ok(held) => {
                                eprintln!(
                                    "global voice: keyboard events dropped (UI stalled); resynced, {}",
                                    match (capturing, held) {
                                        (true, true) => "hold continues",
                                        (true, false) => "key was released",
                                        _ => "no hold active",
                                    }
                                );
                                if capturing && !held {
                                    self.release(&mut edges);
                                }
                                continue;
                            }
                            Err(error) => {
                                self.devices.remove(&path);
                                self.healthy = false;
                                self.cancel(
                                    &format!(
                                        "keyboard resync after dropped events failed: {error}"
                                    ),
                                    &mut edges,
                                );
                                break;
                            }
                        }
                    }
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(error) => {
                        self.devices.remove(&path);
                        self.healthy = false;
                        self.cancel(&format!("keyboard read failed: {error}"), &mut edges);
                        break;
                    }
                };
                match device.decoder.event(event.kind, event.code, event.value) {
                    Some(edge @ (Edge::Press | Edge::Spawn))
                        if self.capture.is_none() && self.ready() =>
                    {
                        match self.registration.capture(
                            event.time.tv_sec as u64 * 1_000_000_000
                                + event.time.tv_usec as u64 * 1_000,
                        ) {
                            Ok(Some(lock)) => {
                                self.capture = Some((path.clone(), Instant::now(), Some(lock)));
                                edges.push(edge);
                            }
                            Ok(None) => {}
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                self.pending = Some((
                                    path.clone(),
                                    event.time.tv_sec as u64 * 1_000_000_000
                                        + event.time.tv_usec as u64 * 1_000,
                                    edge,
                                ));
                            }
                            Err(_) => {
                                self.registration_ok = false;
                            }
                        }
                    }
                    Some(Edge::Release)
                        if self.capture.as_ref().is_some_and(|(p, _, _)| p == &path) =>
                    {
                        self.release(&mut edges)
                    }
                    _ => {}
                }
            }
        }
        // A short routing-lock collision must not lose a physical press. Drain
        // key-up first so a released key can never become a delayed capture.
        if let Some((path, stamp, edge)) = self.pending.take() {
            if self.capture.is_none()
                && self.ready()
                && self
                    .devices
                    .get(&path)
                    .is_some_and(|d| d.decoder.trigger.is_some())
            {
                match self.registration.capture(stamp) {
                    Ok(Some(lock)) => {
                        self.capture = Some((path, Instant::now(), Some(lock)));
                        edges.push(edge);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        self.pending = Some((path, stamp, edge))
                    }
                    Err(_) => self.registration_ok = false,
                    Ok(None) => {}
                }
            }
        }
        // After draining, so a key-up during startup is already applied.
        self.adopt(&mut edges);
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "jcode-voice-test-{}-{}-{}",
            std::process::id(),
            now().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[test]
    fn decoder_chords_repeat_and_release() {
        let mut d = Decoder::default();
        assert_eq!(d.event(1, F23 as u16, 1), None);
        d.event(1, F23 as u16, 0);
        // Hardware order: synthetic Meta, then Shift, then F23.
        d.event(1, 125, 1);
        d.event(1, 42, 1);
        assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Press));
        assert_eq!(d.event(1, F23 as u16, 2), None);
        assert_eq!(d.event(1, F23 as u16, 1), None);
        assert_eq!(d.event(1, 42, 0), None);
        assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
        d.event(1, 125, 0);
        assert_eq!(d.event(1, ASSISTANT as u16, 1), Some(Edge::Press));
        assert_eq!(d.event(1, ASSISTANT as u16, 0), Some(Edge::Release));
        assert_eq!(d.event(1, 30, 1), None);
    }
    #[test]
    fn shift_before_copilot_spawns_but_synthetic_shift_holds() {
        // Copilot alone: synthetic Meta, then Shift, then F23.
        let mut d = Decoder::default();
        d.event(1, 125, 1);
        d.event(1, 42, 1);
        assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Press));
        assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
        // User Shift first. The kernel drops the synthetic duplicate Shift.
        let mut d = Decoder::default();
        d.event(1, 42, 1);
        d.event(1, 125, 1);
        assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Spawn));
        assert_eq!(d.event(1, 42, 0), None);
        assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
        // Right Shift is never synthesized.
        let mut d = Decoder::default();
        d.event(1, 125, 1);
        d.event(1, 42, 1);
        d.event(1, 54, 1);
        assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Spawn));
    }
    #[test]
    fn spawned_window_adopts_a_held_key_or_falls_back_to_a_tap() {
        for held in [true, false] {
            let (mut listener, mut writer, dir) = fake_listener();
            listener.capture = None;
            if held {
                send(&mut writer, 1, 42, 1);
                send(&mut writer, 1, 125, 1);
                // Lost to the opener's routing lock, still physically down.
                let routing = Lock::take(&dir.join("routing.lock")).unwrap();
                send(&mut writer, 1, F23 as u16, 1);
                assert!(listener.poll().is_empty());
                listener.pending = None;
                drop(routing);
            }
            listener.adopt_held_key();
            assert_eq!(
                listener.poll(),
                vec![if held { Edge::Press } else { Edge::Tap }]
            );
            assert!(listener.poll().is_empty(), "adoption happens once");
            if held {
                send(&mut writer, 1, F23 as u16, 0);
                assert_eq!(listener.poll(), vec![Edge::Release]);
            }
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn held_at_open_is_ignored() {
        let mut bits = [0; KEY_BYTES];
        for k in KEYS {
            bits[k / 8] |= 1 << (k % 8);
        }
        let mut d = Decoder::snapshot(&bits);
        assert_eq!(d.event(1, ASSISTANT as u16, 1), None);
        assert_eq!(d.event(1, ASSISTANT as u16, 0), None);
        for key in [29, 97, 56, 100] {
            d.event(1, key, 0);
        }
        assert_eq!(d.event(1, ASSISTANT as u16, 1), Some(Edge::Press));
    }
    #[test]
    fn global_chord_rejects_control_alt_and_partial_synthetic_modifiers() {
        for key in [29, 97, 56, 100, 42, 125] {
            let mut d = Decoder::default();
            d.event(1, key, 1);
            assert_eq!(d.event(1, ASSISTANT as u16, 1), None);
        }
        for path in [
            "/dev/input/event3",
            "/dev/input/by-path/platform-i8042-serio-0-event-kbd",
            "/dev/input/by-id/usb-keyboard-event-kbd",
        ] {
            assert!(allowed_device_path(Path::new(path)));
        }
        for path in [
            "/dev/input/event",
            "/dev/input/mouse0",
            "/dev/input/by-path/../event3",
            "/dev/input/by-id/device",
            "event3",
        ] {
            assert!(!allowed_device_path(Path::new(path)));
        }
    }
    #[test]
    fn newest_focus_and_stable_lease() {
        let dir = temp();
        let mut a = Registration::new(dir.clone()).unwrap();
        let mut b = Registration::new(dir.clone()).unwrap();
        a.update(Some(true), true).unwrap();
        let lease = a.capture(now().unwrap()).unwrap().unwrap();
        b.update(Some(true), true).unwrap();
        assert!(a.capture(now().unwrap()).unwrap().is_none());
        assert!(b.capture(now().unwrap()).unwrap().is_none());
        drop(lease);
        assert!(b.capture(now().unwrap()).unwrap().is_some());
        // Repeated active heartbeats must not steal newer focus.
        a.update(Some(true), true).unwrap();
        assert!(a.capture(now().unwrap()).unwrap().is_none());
        let b_path = b.path.clone();
        drop(b);
        assert!(!b_path.exists());
        assert!(a.capture(now().unwrap()).unwrap().is_some());
        drop(a);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stale_and_unready_registrations_are_excluded() {
        let dir = temp();
        let mut a = Registration::new(dir.clone()).unwrap();
        let mut b = Registration::new(dir.clone()).unwrap();
        a.update(Some(true), true).unwrap();
        b.update(Some(true), false).unwrap();
        assert!(a.capture(now().unwrap()).unwrap().is_some());
        fs::write(&b.path, format!("{} 0 1\n", u64::MAX)).unwrap();
        assert!(a.capture(now().unwrap()).unwrap().is_some());
        drop(a);
        drop(b);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn delayed_press_cannot_recapture_after_focus_changes() {
        let dir = temp();
        let mut a = Registration::new(dir.clone()).unwrap();
        let mut b = Registration::new(dir.clone()).unwrap();
        a.update(Some(true), true).unwrap();
        let stamp = now().unwrap();
        drop(a.capture(stamp).unwrap().unwrap());
        b.update(Some(true), true).unwrap();
        assert!(b.capture(stamp).unwrap().is_none());
        assert!(b.capture(stamp - 1).unwrap().is_none());
        assert!(b.capture(stamp + 1).unwrap().is_some());
        drop(a);
        drop(b);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn capability_bitmap_and_native_abi() {
        let mut words = [0usize; KEY_BYTES / size_of::<usize>()];
        for key in [F23, ASSISTANT] {
            words[key / usize::BITS as usize] |= 1 << (key % usize::BITS as usize);
        }
        let text = words
            .iter()
            .rev()
            .map(|w| format!("{w:x}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(advertised(&text, F23).unwrap());
        assert!(advertised(&text, ASSISTANT).unwrap());
        assert!(!advertised(&text, 30).unwrap());
        assert!(advertised("invalid", F23).is_err());
        assert_eq!(size_of::<InputEvent>(), size_of::<libc::timeval>() + 8);
        assert_eq!(size_of::<InputMask>(), 16);
        assert_eq!(request(true, 0x93, 16), 0x40104593);
    }
    fn fake_listener() -> (Listener, File, PathBuf) {
        use std::os::fd::FromRawFd;
        let dir = temp();
        let mut fds = [-1; 2];
        // SAFETY: writable two-fd array. Only a private anonymous pipe, no devices.
        assert_eq!(
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
            0
        );
        let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        let mut listener = Listener {
            registration: Registration::new(dir.clone()).unwrap(),
            allowlist: Vec::new(),
            eligible: true,
            devices: BTreeMap::from([(
                PathBuf::from("fixture"),
                Device {
                    file: read,
                    decoder: Decoder::default(),
                },
            )]),
            capture: None,
            pending: None,
            adopt_until: None,
            next_scan: Instant::now() + Duration::from_secs(1000),
            healthy: true,
            registration_ok: false,
        };
        listener.set_active(true);
        (listener, write, dir)
    }
    fn send(file: &mut File, kind: u16, code: u16, value: i32) {
        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        // SAFETY: initialized native ABI struct, read-only for its exact size.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const InputEvent).cast::<u8>(),
                size_of::<InputEvent>(),
            )
        };
        file.write_all(bytes).unwrap();
    }
    #[test]
    fn dropped_events_resync_keeps_a_held_key_and_releases_a_lifted_one() {
        for still_held in [true, false] {
            let (mut listener, mut writer, dir) = fake_listener();
            send(&mut writer, 1, ASSISTANT as u16, 1);
            assert_eq!(listener.poll(), vec![Edge::Press]);
            let decoder = &mut listener
                .devices
                .get_mut(Path::new("fixture"))
                .unwrap()
                .decoder;
            let mut held = [0u8; KEY_BYTES];
            if still_held {
                held[ASSISTANT / 8] |= 1 << (ASSISTANT % 8);
            }
            assert_eq!(decoder.resync(&held), still_held);
            assert_eq!(decoder.trigger.is_some(), still_held);
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn poll_dropped_disconnect_deadline_and_drop() {
        for reason in 0..4 {
            let (mut listener, mut writer, dir) = fake_listener();
            assert!(listener.ready());
            send(&mut writer, 1, ASSISTANT as u16, 1);
            assert_eq!(listener.poll(), vec![Edge::Press]);
            assert!(Lock::take(&dir.join("capture.lock")).is_err());
            match reason {
                0 => send(&mut writer, 0, 3, 0),
                1 => drop(writer),
                2 => listener.capture.as_mut().unwrap().1 = Instant::now() - DEADLINE,
                _ => {
                    let path = listener.registration.path.clone();
                    drop(listener);
                    assert!(!path.exists());
                    assert!(Lock::take(&dir.join("capture.lock")).is_ok());
                    fs::remove_dir_all(dir).unwrap();
                    continue;
                }
            }
            assert_eq!(listener.poll(), vec![Edge::Cancel]);
            assert!(listener.poll().is_empty());
            assert!(Lock::take(&dir.join("capture.lock")).is_ok());
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn eligibility_cancels_but_physical_key_up_releases() {
        for eligible in [false, true] {
            let (mut listener, mut writer, dir) = fake_listener();
            send(&mut writer, 1, ASSISTANT as u16, 1);
            assert_eq!(listener.poll(), vec![Edge::Press]);
            if eligible {
                send(&mut writer, 1, ASSISTANT as u16, 0);
                assert_eq!(listener.poll(), vec![Edge::Release]);
            } else {
                listener.set_eligible(false);
                assert!(!listener.ready());
                assert_eq!(listener.poll(), vec![Edge::Cancel]);
                listener.set_eligible(true);
                send(&mut writer, 1, ASSISTANT as u16, 2);
                assert!(listener.poll().is_empty());
            }
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn registry_failure_cancels_and_invalid_allowlist_fails() {
        let (mut listener, mut writer, dir) = fake_listener();
        send(&mut writer, 1, ASSISTANT as u16, 1);
        assert_eq!(listener.poll(), vec![Edge::Press]);
        fs::remove_file(dir.join("routing.lock")).unwrap();
        fs::create_dir(dir.join("routing.lock")).unwrap();
        listener.registration.published = None;
        assert_eq!(listener.poll(), vec![Edge::Cancel]);
        assert!(!listener.ready());
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
        // Rejected before looking up XDG_RUNTIME_DIR or opening anything.
        assert!(Listener::new(&[PathBuf::from("/dev/input/mouse0")]).is_err());
        assert!(Listener::new(&[PathBuf::from("/dev/input/by-id/device")]).is_err());
    }
    #[test]
    fn routing_contention_retries_only_while_physical_key_remains_down() {
        for release in [false, true] {
            let (mut listener, mut writer, dir) = fake_listener();
            let routing = Lock::take(&dir.join("routing.lock")).unwrap();
            send(&mut writer, 1, ASSISTANT as u16, 1);
            assert!(listener.poll().is_empty());
            assert!(listener.pending.is_some());
            if release {
                send(&mut writer, 1, ASSISTANT as u16, 0);
            }
            drop(routing);
            assert_eq!(
                listener.poll(),
                if release { vec![] } else { vec![Edge::Press] }
            );
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn heartbeat_is_throttled_but_focus_and_eligibility_publish_immediately() {
        let dir = temp();
        let mut registration = Registration::new(dir.clone()).unwrap();
        registration.update(Some(true), true).unwrap();
        let first = fs::read(&registration.path).unwrap();
        registration.update(Some(true), true).unwrap();
        assert_eq!(fs::read(&registration.path).unwrap(), first);
        registration.update(None, false).unwrap();
        assert_ne!(fs::read(&registration.path).unwrap(), first);
        drop(registration);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unsafe_directory_is_rejected() {
        let dir = temp();
        fs::create_dir(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Registration::new(dir.clone()).is_err());
        fs::remove_dir(dir).unwrap();
    }
}
