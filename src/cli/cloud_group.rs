//! Coordinated, resumable handoff. A durable source journal is written before
//! fencing members. Once activation is attempted, leases are NEVER rolled back:
//! an SSH failure cannot prove the receiver did not take ownership.
use super::*;
use std::collections::BTreeSet;

include!("cloud_group_run.rs");
include!("cloud_group_receive.rs");
include!("cloud_group_daemon.rs");

#[cfg(test)]
#[path = "cloud_group_tests.rs"]
mod tests;

const GROUP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Member {
    manifest: MoveManifest,
    session_sha256: String,
    todos_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Group {
    version: u32,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    excluded_session_ids: Vec<String>,
    coordinator: String,
    members: Vec<Member>,
    // Portable swarm persistence, not live process IDs or sockets.
    swarm: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Preparing,
    Activating,
    Complete,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    group: Group,
    previous: Vec<Option<storage::SessionLease>>,
    phase: Phase,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct VerifiedMember {
    id: String,
    epoch: u64,
    session_sha256: String,
    todos_sha256: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GroupReceipt {
    version: u32,
    coordinator: String,
    members: Vec<VerifiedMember>,
    snapshot: Option<String>,
    manifest_sha256: String,
}

fn checked_id(id: &str) -> Result<()> {
    baseline_path(id).map(|_| ())
}

fn group_dir(id: &str) -> Result<PathBuf> {
    checked_id(id)?;
    Ok(storage::jcode_dir()?.join("cloud_groups").join(id))
}

fn member_marker(id: &str) -> Result<PathBuf> {
    checked_id(id)?;
    Ok(storage::jcode_dir()?
        .join("cloud_group_members")
        .join(format!("{id}.json")))
}

pub(super) fn reject_member_return(id: &str) -> Result<()> {
    if member_marker(id)?.exists() {
        bail!(
            "session {id} belongs to a coordinated cloud migration; single-session move/return is unsafe and group return is not implemented"
        );
    }
    Ok(())
}

pub(super) fn is_group_root(id: &str, epoch: u64) -> Result<bool> {
    let path = group_dir(id)?.join("received.json");
    if !path.exists() {
        return Ok(false);
    }
    let group: Group = read_json(&path)?;
    Ok(group.coordinator == id && root(&group)?.manifest.epoch == epoch)
}

/// OS locks are released on process death, unlike lock-directory sentinels.
/// All group and single-session mutators acquire the same ordered locks.
pub(super) fn lock_sessions(ids: &[String]) -> Result<Vec<std::fs::File>> {
    let directory = storage::jcode_dir()?.join("cloud_migration_locks");
    std::fs::create_dir_all(&directory)?;
    let mut locks = Vec::new();
    for id in ids.iter().collect::<BTreeSet<_>>() {
        checked_id(id)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{id}.lock")))?;
        file.try_lock()
            .with_context(|| format!("another migration is changing session {id}"))?;
        locks.push(file);
    }
    Ok(locks)
}

fn root(group: &Group) -> Result<&Member> {
    group
        .members
        .iter()
        .find(|m| m.manifest.session_id == group.coordinator)
        .context("group coordinator is not included")
}

fn digest(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

fn receipt(group: &Group) -> Result<GroupReceipt> {
    Ok(GroupReceipt {
        version: GROUP_VERSION,
        coordinator: group.coordinator.clone(),
        members: group
            .members
            .iter()
            .map(|m| VerifiedMember {
                id: m.manifest.session_id.clone(),
                epoch: m.manifest.epoch,
                session_sha256: m.session_sha256.clone(),
                todos_sha256: m.todos_sha256.clone(),
            })
            .collect(),
        snapshot: root(group)?
            .manifest
            .repo
            .as_ref()
            .map(|r| r.snapshot.clone()),
        manifest_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(group)?)),
    })
}

fn validate(group: &Group, stage: &Path) -> Result<()> {
    if group.version != GROUP_VERSION || group.members.is_empty() {
        bail!("unsupported or empty coordinated migration");
    }
    checked_id(&group.coordinator)?;
    let coordinator = root(group)?;
    let bundle: crate::server::GroupTransferBundle = serde_json::from_value(group.swarm.clone())?;
    if bundle.version != GROUP_VERSION
        || bundle.destination_host != coordinator.manifest.target_host
        || bundle.sessions.len() != group.members.len()
        || group.members.iter().any(|m| {
            !bundle
                .sessions
                .iter()
                .any(|s| s.session_id == m.manifest.session_id && s.epoch == m.manifest.epoch)
        })
    {
        bail!("swarm transfer bundle does not match group membership and epochs");
    }
    let repo = coordinator
        .manifest
        .repo
        .as_ref()
        .context("group migration requires one repository")?;
    let mut ids = BTreeSet::new();
    for member in &group.members {
        let manifest = &member.manifest;
        checked_id(&manifest.session_id)?;
        if !ids.insert(&manifest.session_id)
            || manifest.epoch == 0
            || manifest.format_version != FORMAT_VERSION
        {
            bail!("invalid group member identity, epoch or version");
        }
        let member_repo = manifest
            .repo
            .as_ref()
            .context("group member has no repository")?;
        if member_repo.root != repo.root
            || member_repo.snapshot != repo.snapshot
            || manifest.target_host != coordinator.manifest.target_host
        {
            bail!("group members do not share a repository snapshot and target");
        }
        let directory = stage.join("members").join(&manifest.session_id);
        let sess: session::Session = read_json(&directory.join("session.json"))?;
        if sess.id != manifest.session_id
            || sess.migration_epoch != manifest.epoch
            || digest(&directory.join("session.json"))? != member.session_sha256
        {
            bail!(
                "group session integrity mismatch for {}",
                manifest.session_id
            );
        }
        let todos = directory.join("todos.json");
        match &member.todos_sha256 {
            Some(expected) if digest(&todos)? == *expected => {}
            None if !todos.exists() => {}
            _ => bail!("group todo integrity mismatch for {}", manifest.session_id),
        }
    }
    Ok(())
}

fn save_journal(directory: &Path, journal: &Journal) -> Result<()> {
    durable_json(&directory.join("journal.json"), journal)
}

fn durable_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("missing journal parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn mark_members(group: &Group) -> Result<()> {
    for member in &group.members {
        durable_json(
            &member_marker(&member.manifest.session_id)?,
            &serde_json::json!({
                "coordinator": group.coordinator, "epoch": member.manifest.epoch,
                "host": member.manifest.target_host,
            }),
        )?;
    }
    Ok(())
}

fn member_lease(member: &Member, away: Option<String>) -> storage::SessionLease {
    let manifest = &member.manifest;
    storage::SessionLease {
        session_id: manifest.session_id.clone(),
        epoch: manifest.epoch,
        away_host: away,
        repo_root: manifest.repo.as_ref().map(|r| r.root.clone()),
        last_sync_snapshot: manifest.repo.as_ref().map(|r| r.snapshot.clone()),
        last_sync_head: manifest.repo.as_ref().map(|r| r.head.clone()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn restore_preparing(journal: &Journal) -> Result<()> {
    if journal.phase != Phase::Preparing {
        bail!("cannot restore source leases after cloud activation may have begun");
    }
    // An earlier prepare RPC may still be finishing after losing its reply.
    // The daemon's admission reservation serializes abort behind it, and checks
    // that every lease is either our exact fence or the unchanged prior lease.
    daemon_command("abort", &journal.group.swarm)?;
    for member in &journal.group.members {
        let marker = member_marker(&member.manifest.session_id)?;
        if marker.exists() {
            std::fs::remove_file(marker)?;
        }
    }
    Ok(())
}

fn verify_preview_repo(group: &Group, stage: &Path) -> Result<()> {
    let repo = root(group)?
        .manifest
        .repo
        .as_ref()
        .context("preview repository missing")?;
    let temporary = tempfile::tempdir()?;
    let directory = if Path::new(&repo.root).join(".git").exists() {
        Path::new(&repo.root)
    } else {
        git(temporary.path(), &["init", "--quiet"])?;
        temporary.path()
    };
    let bundle = stage.join("repo.bundle");
    let bundle = bundle.to_str().context("non-UTF8 bundle path")?;
    git(directory, &["bundle", "verify", bundle])?;
    let heads = git(directory, &["bundle", "list-heads", bundle])?;
    if !heads
        .lines()
        .any(|line| line.split_whitespace().next() == Some(repo.snapshot.as_str()))
    {
        bail!("preview repository bundle does not advertise the group snapshot");
    }
    Ok(())
}

fn group_arrival_notice(manifest: &MoveManifest, coordinator: &str, members: &[String]) -> String {
    let mut lines = arrival_context(manifest);
    lines.push(format!("Coordinated migration members: {}. Swarm ownership, task assignments and todos came along. Group return is not implemented; do not return individual members.", members.join(", ")));
    lines.push(crate::cloud_environment::group_migration_guidance(
        &manifest.session_id,
        coordinator,
    ));
    if manifest.session_id == coordinator {
        lines.push("Workers are paused after migration. First verify and complete shared repository/runtime/environment setup once. Then explicitly resume the migrated workers with the swarm tool, preserving their existing session IDs and task assignments. Do not respawn duplicate workers or resume them before setup is ready. Report any blocker honestly.".into());
    } else {
        lines.push(format!("Remain paused until coordinator {coordinator} confirms shared setup is ready and explicitly resumes your existing session. Do not start duplicate workspace setup or request an independent local resource bridge."));
    }
    lines.join("\n")
}

fn excluded_local_sessions(root: &Path, included: &[String]) -> Result<Vec<String>> {
    excluded_local_sessions_at(&storage::jcode_dir()?.join("sessions"), root, included)
}

fn excluded_local_sessions_at(
    directory: &Path,
    root: &Path,
    included: &[String],
) -> Result<Vec<String>> {
    let mut excluded = BTreeSet::new();
    if !directory.exists() {
        return Ok(Vec::new());
    }
    for entry in std::fs::read_dir(directory)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(value) = read_json::<serde_json::Value>(&path) else {
            continue;
        };
        let (Some(id), Some(working_dir)) = (value["id"].as_str(), value["working_dir"].as_str())
        else {
            continue;
        };
        if included.iter().any(|included| included == id)
            || checked_id(id).is_err()
            || storage::read_session_lease(id).is_some_and(|lease| !lease.is_here())
        {
            continue;
        }
        let Ok(canonical) = Path::new(working_dir).canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) {
            continue;
        }
        let Ok(candidate) = git(&canonical, &["rev-parse", "--show-toplevel"]) else {
            continue;
        };
        if Path::new(&candidate).canonicalize().ok().as_deref() == Some(root) {
            excluded.insert(id.to_owned());
        }
    }
    Ok(excluded.into_iter().collect())
}
