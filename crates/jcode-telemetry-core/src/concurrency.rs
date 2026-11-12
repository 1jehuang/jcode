//! Concurrency belongs to live runtime Agent owners, not the process-global
//! activity accumulator. A server can own many Agents and a TUI owns none.
//!
//! One guard represents one runtime incarnation, including idle/background
//! Agents, until explicit finish or drop. Parent presence distinguishes children
//! (manual splits and automated agents).
//! Counts cover v2 participants sharing a local JCODE_HOME, not other machines,
//! old clients, connected viewers, or persisted sessions. No session ID is used
//! as a filesystem path. OS leases have no clock/mtime/PID-reuse dependency.
//!
//! The registry lock serializes joins, samples and departures. Every join updates
//! every live owner's independent high-water marks, including idle owners. A
//! crash releases its lease without requiring a final event. Crashes during the
//! registry commit cannot partially update peers' peaks: one atomic snapshot
//! publishes the join. Filesystem errors make samples unavailable rather than
//! manufacturing concurrency numbers. This is not a power-loss durable log.
//! An unpublished lease invalidates overlapping owners until a quiet point,
//! even after the failed owner exits. Failures before a lease can be created
//! cannot notify peers: this is participating-owner telemetry, not an inventory
//! of opted-out Agents or owners inaccessible through this filesystem.
use super::*;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const TRACKING_VERSION: u32 = 2;
const TRACKING_SCOPE: &str = "runtime_agent_sessions";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    total: u32,
    root: u32,
    child: u32,
}

impl Counts {
    fn add(&mut self, child: bool) {
        self.total = self.total.saturating_add(1);
        if child {
            self.child = self.child.saturating_add(1);
        } else {
            self.root = self.root.saturating_add(1);
        }
    }

    fn observe(&mut self, other: Self) {
        self.total = self.total.max(other.total);
        self.root = self.root.max(other.root);
        self.child = self.child.max(other.child);
    }
}

#[derive(Debug)]
struct Record {
    child: bool,
    peak: Counts,
}

impl Record {
    fn read(value: &Value) -> io::Result<Self> {
        let number = |key| {
            value[key]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| io::Error::other("invalid concurrency record"))
        };
        let child = value["child"]
            .as_bool()
            .ok_or_else(|| io::Error::other("invalid concurrency role"))?;
        let peak = Counts {
            total: number("total")?,
            root: number("root")?,
            child: number("children")?,
        };
        if peak.total == 0
            || peak.root > peak.total
            || peak.child > peak.total
            || u64::from(peak.root) + u64::from(peak.child) < u64::from(peak.total)
            || (child && peak.child == 0)
            || (!child && peak.root == 0)
        {
            return Err(io::Error::other("inconsistent concurrency peak record"));
        }
        Ok(Self { child, peak })
    }

    fn value(&self) -> Value {
        serde_json::json!({
            "child": self.child, "total": self.peak.total,
            "root": self.peak.root, "children": self.peak.child,
        })
    }
}

fn read_snapshot(dir: &Path) -> io::Result<serde_json::Map<String, Value>> {
    match std::fs::read(dir.join("registry.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error),
    }
}

fn write_snapshot(dir: &Path, records: &serde_json::Map<String, Value>) -> io::Result<()> {
    atomic_private_write(&dir.join("registry.json"), &serde_json::to_vec(records)?)
}

pub(super) fn atomic_private_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = private_file(&temporary, false)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(temporary, path)
}

fn private_file(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Never delete the registry lock file: replacing its inode would create two
/// independent locks. A bounded wait keeps telemetry from hanging startup.
fn lock_registry(dir: &Path) -> io::Result<FileLock> {
    std::fs::create_dir_all(dir)?;
    lock_path(&dir.join("registry.lock"))
}

pub(super) struct FileLock(File);

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(super) fn lock_path(path: &Path) -> io::Result<FileLock> {
    let file = private_file(path, false)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(FileLock(file)),
            Err(TryLockError::WouldBlock) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "concurrency registry busy",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Only call while holding registry.lock. Successfully acquiring a lease proves
/// that its owner has exited; a timestamp or a reused PID never proves liveness.
fn live_records(dir: &Path, exclude: Option<&Path>) -> io::Result<Vec<(String, Record)>> {
    let mut live = Vec::new();
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if Some(path.as_path()) == exclude
            || !entry.file_type()?.is_file()
            || path.extension().and_then(|s| s.to_str()) != Some("lease")
            || path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .is_none()
        {
            continue;
        }
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        match file.try_lock() {
            Ok(()) => {
                file.unlock()?;
                drop(file);
                stale.push(path);
            }
            Err(TryLockError::WouldBlock) => {
                live.push(entry.file_name().to_string_lossy().into_owned())
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    // An unpublished lease is a degraded-coverage signal, even AFTER its owner
    // exits. Retain it until a quiet point, otherwise an idle surviving owner
    // could report an understated peak after a short-lived failed join.
    // New owners during a degraded epoch also retain unpublished leases.
    let snapshot = if live.is_empty() {
        Default::default()
    } else {
        read_snapshot(dir)?
    };
    if !live.is_empty()
        && stale.iter().any(|path| {
            !snapshot.contains_key(path.file_name().unwrap().to_string_lossy().as_ref())
        })
    {
        return Err(io::Error::other(
            "concurrency coverage degraded until all owners exit",
        ));
    }
    let records = live
        .into_iter()
        .map(|key| {
            let value = snapshot
                .get(&key)
                .ok_or_else(|| io::Error::other("live concurrency record missing"))?;
            Ok((key, Record::read(value)?))
        })
        .collect::<io::Result<Vec<_>>>()?;
    for path in stale {
        std::fs::remove_file(path)?;
    }
    Ok(records)
}

#[derive(Debug)]
struct Lease {
    dir: PathBuf,
    path: PathBuf,
    file: Option<File>,
}

impl Lease {
    fn release(&mut self) -> io::Result<()> {
        // Closing alone can leave a flock held by a concurrently forked child
        // until it execs. Explicit unlock ends clean ownership immediately,
        // including for inherited copies of the open file description.
        self.file.take().map_or(Ok(()), |file| file.unlock())
    }

    fn begin(dir: PathBuf, incarnation: &str, child: bool) -> io::Result<(Self, Option<Counts>)> {
        let _registry = lock_registry(&dir)?;
        let path = dir.join(format!("{incarnation}.lease"));
        let file = private_file(&path, true)?;
        file.lock()?;
        let result = (|| {
            let records = live_records(&dir, Some(&path))?;
            let mut counts = Counts::default();
            for (_, record) in &records {
                counts.add(record.child);
            }
            counts.add(child);
            let mut snapshot = serde_json::Map::new();
            snapshot.insert(
                format!("{incarnation}.lease"),
                Record {
                    child,
                    peak: counts,
                }
                .value(),
            );
            for (key, mut record) in records {
                record.peak.observe(counts);
                snapshot.insert(key, record.value());
            }
            write_snapshot(&dir, &snapshot)?;
            Ok::<_, io::Error>(counts)
        })();
        // Keep ownership on publication failure. Its missing snapshot record
        // invalidates peer samples, including after this failed owner exits.
        Ok((
            Self {
                dir,
                path,
                file: Some(file),
            },
            result.ok(),
        ))
    }

    fn finish(&mut self) -> io::Result<Counts> {
        let registry = lock_registry(&self.dir);
        // Even an unavailable registry must not retain a live lease after the
        // Agent finishes. Its unlocked marker will be pruned by the next join.
        if let Err(error) = registry {
            let _ = self.release();
            return Err(error);
        }
        let _registry = registry?;
        let peak = (|| {
            live_records(&self.dir, None)?;
            let mut snapshot = read_snapshot(&self.dir)?;
            let key = self.path.file_name().unwrap().to_string_lossy();
            let value = snapshot
                .remove(key.as_ref())
                .ok_or_else(|| io::Error::other("concurrency record missing at finish"))?;
            let peak = Record::read(&value)?.peak;
            write_snapshot(&self.dir, &snapshot)?;
            Ok(peak)
        })();
        let released = self.release();
        if peak.is_ok() && released.is_ok() {
            let _ = std::fs::remove_file(&self.path);
        }
        released?;
        peak
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = self.finish();
        }
    }
}

/// One live runtime Agent incarnation. Hold this on the Agent, not its viewers
/// or the process-global activity accumulator. Call `finish` when a retained
/// Agent closes/crashes, and create a new guard when it starts a new session.
#[derive(Debug)]
pub struct ConcurrencySession {
    session_id: String,
    incarnation: String,
    child: bool,
    lease: Option<Lease>,
    at_start: Counts,
    active: bool,
}

/// Begin telemetry ownership for a live Agent. Disabled telemetry creates an
/// inert guard and performs no filesystem operations or event delivery.
pub fn begin_concurrency_session(
    session_id: &str,
    parent_session_id: Option<&str>,
) -> ConcurrencySession {
    let mut guard = ConcurrencySession {
        session_id: session_id.to_owned(),
        incarnation: uuid::Uuid::new_v4().to_string(),
        child: parent_session_id.is_some(),
        lease: None,
        at_start: Counts::default(),
        active: is_enabled(),
    };
    if !guard.active {
        return guard;
    }
    let result = storage::jcode_dir().and_then(|dir| {
        Lease::begin(
            dir.join("telemetry_concurrency_v2"),
            &guard.incarnation,
            guard.child,
        )
        .map_err(Into::into)
    });
    if let Ok((lease, counts)) = result {
        guard.lease = Some(lease);
        guard.at_start = counts.unwrap_or_default();
    }
    guard.emit(
        "start",
        (guard.at_start.total > 0).then_some(guard.at_start),
        DeliveryMode::Background,
    );
    guard
}

impl ConcurrencySession {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// End once, even if the Agent is retained or Drop subsequently runs.
    /// Explicit close attempts bounded blocking delivery; Drop only queues a
    /// best-effort event and may lose it at process exit. Neither is durable.
    pub fn finish(&mut self) {
        self.finish_with_mode(DeliveryMode::Blocking(BLOCKING_LIFECYCLE_TIMEOUT));
    }

    fn finish_with_mode(&mut self, mode: DeliveryMode) {
        if !self.active {
            return;
        }
        self.active = false;
        let peak = self
            .lease
            .take()
            .and_then(|mut lease| lease.finish().ok())
            .filter(|peak| {
                self.at_start.total > 0
                    && peak.total >= self.at_start.total
                    && peak.root >= self.at_start.root
                    && peak.child >= self.at_start.child
            });
        self.emit("end", peak, mode);
    }

    fn emit(&self, phase: &str, peak: Option<Counts>, mode: DeliveryMode) {
        if !is_enabled() {
            return;
        }
        let Some(id) = get_or_create_id() else { return };
        let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
        let mut payload = serde_json::json!({
            "event": "session_concurrency", "phase": phase,
            "event_id": new_event_id(), "id": id,
            "session_id": self.session_id, "concurrency_session_id": self.incarnation,
            "concurrency_tracking_version": TRACKING_VERSION,
            "concurrency_tracking_scope": TRACKING_SCOPE,
            "concurrency_tracking_available": peak.is_some(),
            "agent_role": if self.child { "child" } else { "root" },
            "version": version(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "schema_version": schema_version, "build_channel": build_channel,
            "is_git_checkout": git_checkout, "is_ci": ci, "ran_from_cargo": from_cargo,
        });
        if let Some(peak) = peak {
            payload["active_sessions_at_start"] = self.at_start.total.into();
            payload["other_active_sessions_at_start"] =
                self.at_start.total.saturating_sub(1).into();
            payload["root_sessions_at_start"] = self.at_start.root.into();
            payload["child_sessions_at_start"] = self.at_start.child.into();
            payload["max_concurrent_sessions"] = peak.total.into();
            payload["max_concurrent_root_sessions"] = peak.root.into();
            payload["max_concurrent_child_sessions"] = peak.child.into();
            payload["multi_sessioned"] = (peak.total > 1).into();
        }
        let _ = send_payload(payload, mode);
    }
}

impl Drop for ConcurrencySession {
    fn drop(&mut self) {
        self.finish_with_mode(DeliveryMode::Background);
    }
}

/// Legacy lifecycle events cannot attribute logical Agent ownership because
/// their accumulator is a process singleton. Preserve the event, not the old
/// mtime-derived numbers. Dedicated guard events are the v2 source of truth.
pub(super) fn mark_legacy_concurrency_unavailable(payload: &mut Value) {
    if !matches!(
        payload["event"].as_str(),
        Some("session_start" | "session_end" | "session_crash")
    ) {
        return;
    }
    payload["concurrency_tracking_version"] = TRACKING_VERSION.into();
    payload["concurrency_tracking_scope"] = "legacy_process_global".into();
    payload["concurrency_tracking_available"] = false.into();
    if let Some(object) = payload.as_object_mut() {
        for key in [
            "active_sessions_at_start",
            "other_active_sessions_at_start",
            "max_concurrent_sessions",
            "multi_sessioned",
        ] {
            object.remove(key);
        }
    }
}

#[cfg(test)]
mod tests;
