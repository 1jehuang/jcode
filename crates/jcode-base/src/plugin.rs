//! Claude-compatible plugin marketplaces and scoped, explicit install registry.
//! Discovery never fetches repositories or writes configuration.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "plugin/dependencies.rs"]
mod dependencies;

#[path = "plugin/data.rs"]
mod data;
pub use data::{data_path, prepare_data_dir};

static GENERATION: AtomicU64 = AtomicU64::new(0);
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}
pub fn invalidate_cache() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    #[default]
    User,
    Project,
    Local,
}
impl std::fmt::Display for PluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        })
    }
}
impl std::str::FromStr for PluginScope {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            "local" => Ok(Self::Local),
            _ => bail!("invalid plugin scope: {s}"),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginRoot {
    pub name: String,
    pub root: PathBuf,
    pub version: Option<String>,
    pub marketplace: String,
    pub scope: PluginScope,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
    pub root: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailablePlugin {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source: serde_json::Value,
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub root: PathBuf,
    pub scope: PluginScope,
    pub enabled: bool,
    #[serde(default)]
    pub project: Option<PathBuf>,
}
#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    marketplaces: BTreeMap<String, Marketplace>,
    #[serde(default)]
    installed: Vec<InstalledPlugin>,
}
#[derive(Deserialize)]
struct Catalog {
    name: String,
    plugins: Vec<CatalogPlugin>,
    #[serde(default, rename = "allowCrossMarketplaceDependenciesOn")]
    allow_cross_marketplace_dependencies_on: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct CatalogPlugin {
    name: String,
    source: serde_json::Value,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug)]
pub struct PluginManager {
    user_root: PathBuf,
    project_dir: PathBuf,
}
impl PluginManager {
    pub fn new(user_root: PathBuf, project_dir: PathBuf) -> Self {
        Self {
            user_root,
            project_dir: fs::canonicalize(&project_dir).unwrap_or(project_dir),
        }
    }
    pub fn for_dir(dir: &Path) -> Result<Self> {
        let home = crate::storage::jcode_dir()?;
        Ok(Self::new(
            home,
            fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        ))
    }
    fn base(&self) -> PathBuf {
        self.user_root.join("plugins")
    }
    fn ensure_owned_dirs(&self, relative: &Path) -> Result<()> {
        fs::create_dir_all(&self.user_root)?;
        let mut path = self.user_root.canonicalize()?;
        for part in relative.components() {
            ensure!(matches!(part, Component::Normal(_)), "invalid managed path");
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) => ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "managed plugin directory is not a plain directory"
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Err(e) = fs::create_dir(&path) {
                        if e.kind() != std::io::ErrorKind::AlreadyExists {
                            return Err(e.into());
                        }
                        let meta = fs::symlink_metadata(&path)?;
                        ensure!(
                            meta.is_dir() && !meta.file_type().is_symlink(),
                            "managed directory replaced during creation"
                        );
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn mutation_lock(&self) -> Result<fs::File> {
        self.ensure_owned_dirs(Path::new("plugins"))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.base().join("registry.lock"))?;
        file.lock()?;
        Ok(file)
    }
    fn read(&self) -> Result<Registry> {
        read_json_or_default(&self.base().join("registry.json"))
    }
    fn save(&self, r: &Registry) -> Result<()> {
        atomic_json(&self.base().join("registry.json"), r)?;
        invalidate_cache();
        Ok(())
    }
    pub fn list_marketplaces(&self) -> Result<Vec<Marketplace>> {
        Ok(self.read()?.marketplaces.into_values().collect())
    }
    pub fn add_marketplace(&self, source: &str) -> Result<Marketplace> {
        let staged = tempfile::tempdir()?;
        let root = acquire(source, &staged.path().join("source"))?;
        let catalog = read_catalog(&root)?;
        safe_name(&catalog.name)?;
        let _lock = self.mutation_lock()?;
        let mut r = self.read()?;
        ensure!(
            !r.marketplaces.contains_key(&catalog.name),
            "marketplace already registered: {}",
            catalog.name
        );
        let target = self.base().join("marketplaces").join(format!(
            "{}-{}",
            catalog.name,
            uuid::Uuid::new_v4()
        ));
        self.ensure_owned_dirs(Path::new("plugins/marketplaces"))?;
        publish_tree(&root, &target)?;
        let entry = Marketplace {
            name: catalog.name,
            source: if source_is_local(source) {
                fs::canonicalize(source)?.to_string_lossy().into_owned()
            } else {
                source.to_owned()
            },
            root: target,
        };
        r.marketplaces.insert(entry.name.clone(), entry.clone());
        self.save(&r)?;
        Ok(entry)
    }
    pub fn remove_marketplace(&self, name: &str) -> Result<()> {
        let _lock = self.mutation_lock()?;
        let mut r = self.read()?;
        ensure!(
            r.marketplaces.remove(name).is_some(),
            "unknown marketplace: {name}"
        );
        self.save(&r)
    }
    pub fn update_marketplace(&self, name: &str) -> Result<Marketplace> {
        let _lock = self.mutation_lock()?;
        let mut r = self.read()?;
        let old = r
            .marketplaces
            .get(name)
            .context("unknown marketplace")?
            .clone();
        let staged = tempfile::tempdir()?;
        let root = acquire(&old.source, &staged.path().join("source"))?;
        let catalog = read_catalog(&root)?;
        ensure!(catalog.name == name, "marketplace name changed");
        let target =
            self.base()
                .join("marketplaces")
                .join(format!("{}-{}", name, uuid::Uuid::new_v4()));
        self.ensure_owned_dirs(Path::new("plugins/marketplaces"))?;
        publish_tree(&root, &target)?;
        let entry = Marketplace {
            root: target,
            ..old
        };
        r.marketplaces.insert(name.to_owned(), entry.clone());
        self.save(&r)?;
        Ok(entry)
    }
    pub fn discover(&self) -> Result<Vec<AvailablePlugin>> {
        let mut result = Vec::new();
        for m in self.list_marketplaces()? {
            for p in read_catalog(&m.root)?.plugins {
                result.push(AvailablePlugin {
                    name: p.name,
                    marketplace: m.name.clone(),
                    version: p.version,
                    description: p.description,
                    source: p.source,
                    extra: p.extra,
                });
            }
        }
        Ok(result)
    }
    pub fn list(&self) -> Result<Vec<InstalledPlugin>> {
        Ok(self
            .read()?
            .installed
            .into_iter()
            .filter(|p| self.applies(p))
            .collect())
    }
    fn applies(&self, p: &InstalledPlugin) -> bool {
        p.scope == PluginScope::User || p.project.as_deref() == Some(self.project_dir.as_path())
    }
    fn matches(p: &InstalledPlugin, spec: &str, scope: PluginScope) -> bool {
        p.scope == scope && (spec == p.name || spec == format!("{}@{}", p.name, p.marketplace))
    }
    pub fn install(&self, spec: &str, scope: PluginScope) -> Result<InstalledPlugin> {
        self.dependency_transaction(spec, scope, false)
    }
    pub fn uninstall(&self, spec: &str, scope: PluginScope) -> Result<()> {
        let _lock = self.mutation_lock()?;
        let mut r = self.read()?;
        let positions: Vec<_> = r
            .installed
            .iter()
            .enumerate()
            .filter(|(_, p)| self.applies(p) && Self::matches(p, spec, scope))
            .map(|(i, _)| i)
            .collect();
        ensure!(
            positions.len() == 1,
            "plugin not installed or ambiguous: {spec}"
        );
        r.installed.remove(positions[0]);
        self.save(&r)
    }
    fn set_enabled(&self, spec: &str, scope: PluginScope, enabled: bool) -> Result<()> {
        let _lock = self.mutation_lock()?;
        let mut r = self.read()?;
        let matches: Vec<_> = r
            .installed
            .iter_mut()
            .filter(|p| self.applies(p) && Self::matches(p, spec, scope))
            .collect();
        ensure!(
            matches.len() == 1,
            "plugin not installed or ambiguous: {spec}"
        );
        for p in matches {
            p.enabled = enabled;
        }
        self.save(&r)
    }
    pub fn enable(&self, spec: &str, scope: PluginScope) -> Result<()> {
        self.set_enabled(spec, scope, true)
    }
    pub fn disable(&self, spec: &str, scope: PluginScope) -> Result<()> {
        self.set_enabled(spec, scope, false)
    }
    pub fn update(&self, spec: &str, scope: PluginScope) -> Result<InstalledPlugin> {
        self.dependency_transaction(spec, scope, true)
    }
    pub fn validate(path: &Path) -> Result<PluginManifest> {
        let component_root = if path.is_file() {
            let parent = path.parent().context("manifest has no parent")?;
            if parent.file_name().is_some_and(|n| n == ".claude-plugin") {
                parent.parent().unwrap_or(parent).to_path_buf()
            } else {
                parent.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };
        let path = if path.is_file() {
            path.to_path_buf()
        } else {
            path.join(".claude-plugin/plugin.json")
        };
        let manifest: PluginManifest = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
        )?;
        safe_name(&manifest.name)?;
        for key in [
            "commands",
            "skills",
            "agents",
            "hooks",
            "mcpServers",
            "lspServers",
            "outputStyles",
        ] {
            if let Some(value) = manifest.extra.get(key) {
                if key == "commands" && value.is_object() {
                    crate::plugin_components::validate_command_map(&component_root, value)?;
                } else {
                    validate_component_paths(&component_root, value)?;
                }
            }
        }
        dependencies::parse(manifest.extra.get("dependencies"))?;
        Ok(manifest)
    }
    pub fn enabled_roots(&self) -> Result<Vec<PluginRoot>> {
        self.roots_from(self.list()?)
    }
    pub fn enabled_user_roots(&self) -> Result<Vec<PluginRoot>> {
        self.roots_from(
            self.read()?
                .installed
                .into_iter()
                .filter(|p| p.scope == PluginScope::User)
                .collect(),
        )
    }
    fn roots_from(&self, records: Vec<InstalledPlugin>) -> Result<Vec<PluginRoot>> {
        let base = self.base().join("cache");
        let Ok(home) = self.user_root.canonicalize() else {
            return Ok(Vec::new());
        };
        let Ok(base) = base.canonicalize() else {
            return Ok(Vec::new());
        };
        if !base.starts_with(home) {
            return Ok(Vec::new());
        }
        // Apply precedence before checking enabled, so a disabled local record masks user installs.
        let chosen = dependencies::effective_records(records);
        let (satisfied, _) = dependencies::satisfied(&chosen, &base);
        let mut names = BTreeMap::<String, usize>::new();
        for p in chosen.values() {
            *names.entry(p.name.clone()).or_default() += 1;
        }
        Ok(chosen
            .into_values()
            .filter(|p| {
                p.enabled
                    && names.get(&p.name) == Some(&1)
                    && satisfied.contains(&(p.name.clone(), p.marketplace.clone()))
            })
            .filter_map(|p| {
                let root = p.root.canonicalize().ok()?;
                let manifest = Self::validate(&root).ok()?;
                if !root.starts_with(&base) || manifest.name != p.name {
                    return None;
                }
                Some(PluginRoot {
                    name: p.name,
                    root,
                    version: p.version,
                    marketplace: p.marketplace,
                    scope: p.scope,
                })
            })
            .collect())
    }
}
pub fn enabled_user_plugins() -> Vec<PluginRoot> {
    crate::storage::jcode_dir()
        .and_then(|home| PluginManager::new(home, PathBuf::new()).enabled_user_roots())
        .unwrap_or_default()
}
pub fn enabled_plugins_for_dir(dir: &Path) -> Vec<PluginRoot> {
    PluginManager::for_dir(dir)
        .and_then(|m| m.enabled_roots())
        .unwrap_or_default()
}
fn safe_name(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c)),
        "invalid plugin/marketplace name: {s}"
    );
    Ok(())
}
fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(v) => Ok(serde_json::from_slice(&v)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn read_catalog(root: &Path) -> Result<Catalog> {
    let catalog: Catalog =
        serde_json::from_slice(&fs::read(root.join(".claude-plugin/marketplace.json"))?)?;
    safe_name(&catalog.name)?;
    let mut names = std::collections::BTreeSet::new();
    for p in &catalog.plugins {
        safe_name(&p.name)?;
        ensure!(names.insert(&p.name), "duplicate plugin name");
    }
    Ok(catalog)
}
fn acquire(source: &str, target: &Path) -> Result<PathBuf> {
    ensure!(
        !source.chars().any(|c| c.is_control()),
        "invalid repository source"
    );
    let path = Path::new(source);
    if source_is_local(source) {
        let root = path.canonicalize()?;
        return Ok(if root.is_file() {
            root.parent()
                .and_then(Path::parent)
                .context("invalid marketplace path")?
                .to_path_buf()
        } else {
            root
        });
    }
    let url = if source.starts_with("https://")
        || source.starts_with("ssh://")
        || source.starts_with("git@")
    {
        source.to_owned()
    } else {
        let parts: Vec<_> = source.split('/').collect();
        ensure!(
            parts.len() == 2
                && parts.iter().all(|s| !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))),
            "unsupported marketplace source"
        );
        format!("https://github.com/{source}.git")
    };
    ensure!(
        !url.contains('\n') && !url.contains('\0'),
        "invalid repository URL"
    );
    acquire_git(&url, target, None, None)
}

// Callers classify the source before entering this helper. In particular, remote
// objects must never consult Path::exists and accidentally become local sources.
fn acquire_git(
    url: &str,
    target: &Path,
    git_ref: Option<&str>,
    sha: Option<&str>,
) -> Result<PathBuf> {
    validate_git_pin(git_ref, sha)?;
    let mut command = git_command();
    command.args(["clone", "--no-checkout"]);
    // A SHA is authoritative even when a branch/tag is also declared (REA Xbs).
    if sha.is_none() {
        if let Some(git_ref) = git_ref {
            command.args(["--branch", git_ref]);
        }
    }
    let output = command
        .arg("--")
        .arg(url)
        .arg(target)
        .output()
        .context("launching git clone")?;
    ensure!(output.status.success(), "git clone failed");
    let revision = sha.unwrap_or("HEAD");
    if let Some(sha) = sha {
        // Some servers permit fetching a pinned commit not reachable from the
        // advertised branch tips. Never silently substitute the default HEAD.
        if git_output(target, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_err() {
            git_output(target, &["fetch", "--no-tags", "--", "origin", sha])?;
        }
    }
    let commit = git_output(
        target,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    if let Some(sha) = sha {
        ensure!(
            commit.eq_ignore_ascii_case(sha),
            "plugin commit SHA verification failed"
        );
    }
    git_output(target, &["checkout", "--detach", &commit, "--"])?;
    let head = git_output(target, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    ensure!(head == commit, "plugin checkout commit verification failed");
    Ok(target.canonicalize()?)
}

fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    command
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = git_command()
        .current_dir(root)
        .args(args)
        .output()
        .context("launching plugin git operation")?;
    ensure!(output.status.success(), "plugin git {} failed", args[0]);
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn validate_git_pin(git_ref: Option<&str>, sha: Option<&str>) -> Result<()> {
    if let Some(git_ref) = git_ref {
        ensure!(
            !git_ref.is_empty()
                && !git_ref.starts_with('-')
                && !git_ref.chars().any(|c| c.is_whitespace() || c.is_control())
                && !git_ref.contains([':', '~', '^', '?', '*', '[', '\\'])
                && !git_ref.contains("..")
                && !git_ref.contains("@{")
                && !git_ref.ends_with('.')
                && !git_ref.ends_with('/'),
            "unsupported plugin git ref: expected a branch or tag name"
        );
    }
    if let Some(sha) = sha {
        ensure!(sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid plugin commit SHA: expected a full 40-character lowercase hexadecimal commit ID");
    }
    Ok(())
}
fn contained(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "plugin source must stay within marketplace"
    );
    let base = root.canonicalize()?;
    let result = base.join(path).canonicalize()?;
    ensure!(
        result.starts_with(&base),
        "plugin source escapes marketplace"
    );
    Ok(result)
}
fn plugin_source(root: &Path, source: &serde_json::Value, target: &Path) -> Result<PathBuf> {
    if let Some(s) = source.as_str() {
        return contained(root, s);
    }
    let kind = source
        .get("source")
        .and_then(|v| v.as_str())
        .context("invalid plugin source")?;
    let remote = match kind {
        "github" => source.get("repo"),
        "url" | "git-subdir" => source.get("url"),
        _ => bail!("unsupported plugin source: {kind}"),
    }
    .and_then(|v| v.as_str())
    .context("missing plugin repository")?;
    let object = source
        .as_object()
        .context("plugin source must be a string or object")?;
    let allowed: &[&str] = match kind {
        "github" => &["source", "repo", "ref", "sha"],
        "url" => &["source", "url", "ref", "sha"],
        "git-subdir" => &["source", "url", "path", "ref", "sha"],
        _ => bail!("unsupported plugin source: {kind}"),
    };
    for key in object.keys() {
        ensure!(allowed.contains(&key.as_str()),
            "unsupported plugin source option '{key}' for {kind}; subdirectories require source 'git-subdir' with 'path'");
    }
    let string_option = |key: &str| -> Result<Option<&str>> {
        object
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .with_context(|| format!("plugin source '{key}' must be a string"))
            })
            .transpose()
    };
    let git_ref = string_option("ref")?;
    let sha = string_option("sha")?;
    validate_git_pin(git_ref, sha)?;
    let subdir = if kind == "git-subdir" {
        let path = string_option("path")?.context("git-subdir source requires 'path'")?;
        validate_subdir(path)?;
        Some(path)
    } else {
        None
    };
    match kind {
        "github" => ensure!(
            valid_github_repo(remote),
            "github plugin source must be owner/repo"
        ),
        "url" | "git-subdir" => ensure!(
            valid_remote_url(remote) || (kind == "git-subdir" && valid_github_repo(remote)),
            "URL plugin source must be an HTTPS or SSH git repository"
        ),
        _ => unreachable!(),
    }
    let repository = if kind == "github" || (kind == "git-subdir" && valid_github_repo(remote)) {
        format!("https://github.com/{remote}.git")
    } else {
        remote.to_owned()
    };
    // Remote object provenance is authoritative, even if a URL-shaped local
    // directory happens to exist in the process working directory.
    let checkout = acquire_git(&repository, target, git_ref, sha)?;
    match subdir {
        Some(path) => git_subdir(&checkout, path),
        None => Ok(checkout),
    }
}

fn validate_subdir(relative: &str) -> Result<()> {
    let path = Path::new(relative);
    ensure!(
        !relative.is_empty()
            && !relative.contains('\\')
            && !relative.chars().any(|c| c.is_control())
            && !path.is_absolute()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "git-subdir path must stay within repository"
    );
    Ok(())
}

fn git_subdir(checkout: &Path, relative: &str) -> Result<PathBuf> {
    validate_subdir(relative)?;
    let base = checkout.canonicalize()?;
    let mut cursor = base.clone();
    for component in Path::new(relative).components() {
        cursor.push(component.as_os_str());
        ensure!(
            !fs::symlink_metadata(&cursor)?.file_type().is_symlink(),
            "git-subdir path must not traverse symlinks"
        );
    }
    let result = contained(&base, relative)?;
    ensure!(result.is_dir(), "git-subdir path must be a directory");
    Ok(result)
}
fn valid_github_repo(source: &str) -> bool {
    let parts: Vec<_> = source.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && !part.starts_with('-')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
fn source_is_local(source: &str) -> bool {
    !valid_remote_url(source) && Path::new(source).exists()
}
fn valid_remote_url(source: &str) -> bool {
    (source.starts_with("https://") || source.starts_with("ssh://") || source.starts_with("git@"))
        && !source.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn effective_manifest(
    source: &Path,
    name: &str,
    version: Option<String>,
    description: Option<String>,
    extra: &BTreeMap<String, serde_json::Value>,
) -> Result<PluginManifest> {
    let path = source.join(".claude-plugin/plugin.json");
    let mut manifest = if path.exists() {
        PluginManager::validate(source)?
    } else {
        PluginManifest {
            name: name.to_owned(),
            version: version.clone(),
            description: description.clone(),
            extra: BTreeMap::new(),
        }
    };
    ensure!(
        manifest.name == name,
        "plugin manifest name does not match marketplace"
    );
    manifest.version = manifest.version.or(version);
    manifest.description = manifest.description.or(description);
    for (key, value) in extra {
        match key.as_str() {
            "strict" | "defaultEnabled" | "category" | "tags" => continue,
            "dependencies" => {
                dependencies::parse(Some(value))?;
            }
            _ => {}
        }
        if let Some(existing) = manifest.extra.get_mut(key) {
            if existing == value {
                continue;
            }
            if let (Some(left), Some(right)) = (existing.as_array_mut(), value.as_array()) {
                left.extend(right.iter().cloned());
            } else {
                bail!("conflicting marketplace and manifest field: {key}");
            }
        } else {
            manifest.extra.insert(key.clone(), value.clone());
        }
    }
    for key in [
        "commands",
        "skills",
        "agents",
        "hooks",
        "mcpServers",
        "lspServers",
        "outputStyles",
    ] {
        if let Some(value) = manifest.extra.get(key) {
            if key == "commands" && value.is_object() {
                crate::plugin_components::validate_command_map(source, value)?;
            } else {
                validate_component_paths(source, value)?;
            }
        }
    }
    Ok(manifest)
}
fn validate_component_paths(root: &Path, value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::String(path) => {
            let path = path.strip_prefix("${CLAUDE_PLUGIN_ROOT}/").unwrap_or(path);
            contained(root, path)?;
        }
        serde_json::Value::Array(paths) => {
            for path in paths {
                validate_component_paths(root, path)?;
            }
        }
        // Inline hook/MCP/LSP configuration contains commands, not component paths.
        serde_json::Value::Object(_) => {}
        _ => bail!("invalid component path declaration"),
    }
    Ok(())
}
fn publish_tree(source: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().context("missing copy destination parent")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    let content = stage.path().join("content");
    copy_tree(source, &content)?;
    ensure!(!target.exists(), "copy destination exists");
    fs::rename(content, target)?;
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(source)?.file_type().is_symlink(),
        "symlink plugin content rejected"
    );
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let kind = entry.file_type()?;
        ensure!(!kind.is_symlink(), "symlink plugin content rejected");
        let dest = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?
        } else if kind.is_file() {
            fs::copy(entry.path(), dest)?;
        } else {
            bail!("special file in plugin content")
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "plugin/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "plugin/dependency_tests.rs"]
mod dependency_tests;
