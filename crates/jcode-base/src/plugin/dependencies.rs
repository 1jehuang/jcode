//! Dependency acquisition is atomic at the registry boundary. Missing or untrusted
//! manifest-only edges fail closed, deliberately stricter than Claude's warning path.
use super::*;
use std::collections::{BTreeSet, HashSet};

const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 64;

pub(super) fn parse(value: Option<&serde_json::Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .context("plugin dependencies must be an array")?;
    ensure!(
        array.len() <= MAX_NODES,
        "plugin dependency list exceeds 256 entries"
    );
    let mut result = BTreeSet::new();
    for value in array {
        let id = if let Some(id) = value.as_str() {
            ensure!(
                !id.contains("@^"),
                "unsupported dependency version range: {id}"
            );
            id.to_owned()
        } else if let Some(object) = value.as_object() {
            ensure!(
                object
                    .keys()
                    .all(|k| matches!(k.as_str(), "name" | "marketplace")),
                "unsupported dependency object options; expected name and optional marketplace"
            );
            let name = object
                .get("name")
                .and_then(|v| v.as_str())
                .context("dependency object requires string name")?;
            ensure!(
                valid_identifier(name),
                "invalid dependency object name: {name}"
            );
            match object.get("marketplace") {
                Some(marketplace) => {
                    let marketplace = marketplace
                        .as_str()
                        .context("dependency marketplace must be a string")?;
                    ensure!(
                        valid_identifier(marketplace),
                        "invalid dependency marketplace: {marketplace}"
                    );
                    format!("{name}@{marketplace}")
                }
                None => name.to_owned(),
            }
        } else {
            bail!(
                "unsupported dependency declaration; expected identifier or name/marketplace object"
            )
        };
        let parts: Vec<_> = id.split('@').collect();
        ensure!(
            (1..=2).contains(&parts.len()) && parts.iter().all(|part| valid_identifier(part)),
            "invalid dependency identifier: {id}"
        );
        result.insert(id);
    }
    Ok(result.into_iter().collect())
}

fn valid_identifier(value: &str) -> bool {
    safe_name(value).is_ok()
}

fn qualified(id: &str, declaring_marketplace: &str) -> String {
    if id.contains('@') {
        id.to_owned()
    } else {
        format!("{id}@{declaring_marketplace}")
    }
}

fn root_allowlist(catalog: &Catalog) -> Result<HashSet<String>> {
    let Some(value) = &catalog.allow_cross_marketplace_dependencies_on else {
        return Ok(HashSet::new());
    };
    let values = value
        .as_array()
        .context("allowCrossMarketplaceDependenciesOn must be an array of marketplace names")?;
    ensure!(
        values.len() <= MAX_NODES,
        "dependency marketplace allowlist exceeds 256 entries"
    );
    values
        .iter()
        .map(|value| {
            let name = value
                .as_str()
                .context("dependency marketplace allowlist entries must be strings")?;
            safe_name(name)?;
            Ok(name.to_owned())
        })
        .collect()
}

struct Staged {
    id: String,
    source: PathBuf,
    record: InstalledPlugin,
}

struct Walk<'a> {
    manager: &'a PluginManager,
    registry: &'a Registry,
    stage: &'a Path,
    scope: PluginScope,
    root_marketplace: String,
    root_active: bool,
    root_automatically_disabled: bool,
    allowlist: HashSet<String>,
    visiting: Vec<String>,
    visited: HashSet<String>,
    staged: Vec<Staged>,
}

impl Walk<'_> {
    fn visit(&mut self, id: &str, root: bool) -> Result<()> {
        if let Some(start) = self.visiting.iter().position(|v| v == id) {
            let mut chain = self.visiting[start..].to_vec();
            chain.push(id.to_owned());
            bail!("dependency cycle: {}", chain.join(" -> "));
        }
        if self.visited.contains(id) {
            return Ok(());
        }
        ensure!(
            self.visiting.len() < MAX_DEPTH,
            "dependency graph exceeds depth 64"
        );
        ensure!(
            self.visited.len() + self.visiting.len() < MAX_NODES,
            "dependency graph exceeds 256 nodes"
        );
        let (name, market) = id
            .split_once('@')
            .context("dependency must have a marketplace")?;
        safe_name(name)?;
        safe_name(market)?;
        let rank = |scope| match scope {
            PluginScope::User => 0,
            PluginScope::Project => 1,
            PluginScope::Local => 2,
        };
        if !root {
            if let Some(effective) = self
                .registry
                .installed
                .iter()
                .filter(|p| {
                    self.manager.applies(p)
                        && rank(p.scope) <= rank(self.scope)
                        && p.name == name
                        && p.marketplace == market
                })
                .max_by_key(|p| rank(p.scope))
            {
                ensure!(
                    effective.enabled || effective.automatically_disabled,
                    "dependency {id} is disabled or masked by {} scope; enable dependency first (automatic override of explicit disabled policy is unsupported)",
                    effective.scope
                );
            }
            let existing: Vec<_> = self
                .registry
                .installed
                .iter()
                .filter(|p| {
                    self.manager.applies(p)
                        && p.scope == self.scope
                        && p.name == name
                        && p.marketplace == market
                })
                .collect();
            ensure!(existing.len() <= 1, "ambiguous installed dependency: {id}");
            if let Some(existing) = existing.first() {
                ensure!(
                    existing.enabled || existing.automatically_disabled,
                    "dependency {id} is disabled; enable dependency first (automatic override of explicit disabled policy is unsupported)"
                );
                // Reuse installed bytes and dependency edges, not newer catalog bytes.
                let deps = installed_dependencies(existing, &self.manager.base().join("cache"))?;
                self.visiting.push(id.to_owned());
                for dependency in deps {
                    self.visit(&qualified(&dependency, market), false)?;
                }
                self.visiting.pop();
                self.visited.insert(id.to_owned());
                return Ok(());
            }
            ensure!(
                market == self.root_marketplace || self.allowlist.contains(market),
                "dependency {id} crosses marketplaces and is not in ROOT marketplace allowCrossMarketplaceDependenciesOn; install and enable dependency first"
            );
        }
        ensure!(
            !self.registry.installed.iter().any(|p| {
                self.manager.applies(p)
                    && rank(p.scope) <= rank(self.scope)
                    && p.name == name
                    && p.marketplace != market
            }),
            "another marketplace already supplies this visible plugin name: {name}"
        );
        ensure!(
            !self
                .staged
                .iter()
                .any(|p| p.record.name == name && p.record.marketplace != market),
            "dependency graph supplies conflicting marketplaces for plugin name: {name}"
        );
        let marketplace =
            self.registry.marketplaces.get(market).with_context(|| {
                format!("dependency {id} not found: marketplace is not configured")
            })?;
        let catalog = read_catalog(&marketplace.root)?;
        let plugin = catalog
            .plugins
            .into_iter()
            .find(|p| p.name == name)
            .with_context(|| format!("dependency {id} not found in marketplace"))?;
        let slot = self.stage.join(format!("node-{}", self.staged.len()));
        fs::create_dir(&slot)?;
        let acquired = plugin_source(&marketplace.root, &plugin.source, &slot.join("checkout"))?;
        let manifest = effective_manifest(
            &acquired,
            name,
            plugin.version.clone(),
            plugin.description,
            &plugin.extra,
        )?;
        ensure!(
            manifest.name == name,
            "plugin manifest name does not match marketplace"
        );
        let deps = parse(manifest.extra.get("dependencies"))?;
        let default_enabled = plugin
            .extra
            .get("defaultEnabled")
            .or_else(|| manifest.extra.get("defaultEnabled"));
        if let Some(value) = default_enabled {
            ensure!(value.is_boolean(), "defaultEnabled must be a boolean");
        }
        if root
            && !self.registry.installed.iter().any(|p| {
                self.manager.applies(p)
                    && p.scope == self.scope
                    && p.name == name
                    && p.marketplace == market
            })
        {
            self.root_active = default_enabled.and_then(|v| v.as_bool()).unwrap_or(true);
        }
        // Copy every payload before publishing any registry-visible location.
        let payload = slot.join("payload");
        copy_tree(&acquired, &payload)?;
        atomic_json(&payload.join(".claude-plugin/plugin.json"), &manifest)?;
        let record = InstalledPlugin {
            name: name.to_owned(),
            marketplace: market.to_owned(),
            version: manifest.version.clone().or(plugin.version),
            root: self
                .manager
                .base()
                .join("cache")
                .join(market)
                .join(name)
                .join(uuid::Uuid::new_v4().to_string()),
            scope: self.scope,
            enabled: self.root_active,
            automatically_disabled: if root {
                self.root_automatically_disabled
            } else {
                !self.root_active
            },
            project: if self.scope == PluginScope::User {
                None
            } else {
                Some(self.manager.project_dir.clone())
            },
        };
        self.staged.push(Staged {
            id: id.to_owned(),
            source: payload,
            record,
        });
        self.visiting.push(id.to_owned());
        for dependency in deps {
            self.visit(&qualified(&dependency, market), false)?;
        }
        self.visiting.pop();
        self.visited.insert(id.to_owned());
        Ok(())
    }
}

impl PluginManager {
    pub(super) fn enable_dependency_closure(
        &self,
        registry: &mut Registry,
        root: &InstalledPlugin,
    ) -> Result<()> {
        let base = self.base().join("cache");
        let rank = |scope| match scope {
            PluginScope::User => 0,
            PluginScope::Project => 1,
            PluginScope::Local => 2,
        };
        let mut pending = vec![(root.name.clone(), root.marketplace.clone())];
        let mut seen = HashSet::new();
        while let Some((name, market)) = pending.pop() {
            if !seen.insert((name.clone(), market.clone())) {
                continue;
            }
            ensure!(
                seen.len() <= MAX_NODES,
                "dependency graph exceeds 256 nodes"
            );
            let record = registry
                .installed
                .iter_mut()
                .filter(|p| {
                    self.applies(p)
                        && rank(p.scope) <= rank(root.scope)
                        && p.name == name
                        && p.marketplace == market
                })
                .max_by_key(|p| rank(p.scope))
                .with_context(|| {
                    format!(
                        "dependency {name}@{market} is not installed in a scope visible to {}",
                        root.scope
                    )
                })?;
            if record.automatically_disabled && record.scope == root.scope {
                record.enabled = true;
                record.automatically_disabled = false;
            }
            ensure!(
                record.enabled,
                "dependency {name}@{market} is disabled; enable dependency first"
            );
            for dependency in installed_dependencies(record, &base)? {
                let dependency = qualified(&dependency, &market);
                let (name, market) = dependency
                    .split_once('@')
                    .context("dependency must have a marketplace")?;
                pending.push((name.to_owned(), market.to_owned()));
            }
        }
        let visible = effective_records(
            registry
                .installed
                .iter()
                .filter(|p| self.applies(p) && rank(p.scope) <= rank(root.scope))
                .cloned()
                .collect(),
        );
        let (allowed, diagnostics) = satisfied(&visible, &base);
        ensure!(
            allowed.contains(&(root.name.clone(), root.marketplace.clone())),
            "required dependency closure cannot activate: {}",
            diagnostics.join("; ")
        );
        Ok(())
    }

    /// Read-only diagnostics for enabled records whose effective dependency graph
    /// cannot load. This never acquires sources or changes enablement.
    pub fn dependency_diagnostics(&self) -> Result<Vec<String>> {
        let records = effective_records(self.list()?);
        Ok(satisfied(&records, &self.base().join("cache")).1)
    }

    pub(super) fn dependency_transaction(
        &self,
        spec: &str,
        scope: PluginScope,
        update: bool,
    ) -> Result<InstalledPlugin> {
        let _lock = self.mutation_lock()?;
        let mut registry = self.read()?;
        let old = if update {
            let matches: Vec<_> = registry
                .installed
                .iter()
                .filter(|p| self.applies(p) && Self::matches(p, spec, scope))
                .cloned()
                .collect();
            ensure!(
                matches.len() == 1,
                "plugin not installed or ambiguous: {spec}"
            );
            Some(matches[0].clone())
        } else {
            None
        };
        let id = if let Some(old) = &old {
            format!("{}@{}", old.name, old.marketplace)
        } else {
            let mut matches = Vec::new();
            for market in registry.marketplaces.values() {
                for plugin in read_catalog(&market.root)?.plugins {
                    let id = format!("{}@{}", plugin.name, market.name);
                    if spec == plugin.name || spec == id {
                        matches.push(id);
                    }
                }
            }
            ensure!(matches.len() == 1, "plugin not found or ambiguous: {spec}");
            let id = matches.remove(0);
            ensure!(
                !registry
                    .installed
                    .iter()
                    .any(|p| self.applies(p) && Self::matches(p, &id, scope)),
                "plugin already installed"
            );
            id
        };
        let (_, root_marketplace) = id.split_once('@').context("invalid plugin identity")?;
        let root_catalog = read_catalog(
            &registry
                .marketplaces
                .get(root_marketplace)
                .context("marketplace removed")?
                .root,
        )?;
        let stage = tempfile::tempdir()?;
        let mut walk = Walk {
            manager: self,
            registry: &registry,
            stage: stage.path(),
            scope,
            root_marketplace: root_marketplace.to_owned(),
            root_active: old.as_ref().is_none_or(|p| p.enabled),
            root_automatically_disabled: old.as_ref().is_some_and(|p| p.automatically_disabled),
            allowlist: root_allowlist(&root_catalog)?,
            visiting: Vec::new(),
            visited: HashSet::new(),
            staged: Vec::new(),
        };
        walk.visit(&id, true)?;
        let staged = walk.staged;
        let root = staged
            .iter()
            .find(|p| p.id == id)
            .context("root plugin was not staged")?
            .record
            .clone();
        let mut published = Vec::new();
        let result = (|| -> Result<()> {
            for plugin in staged {
                self.ensure_owned_dirs(
                    &Path::new("plugin-catalogs/cache")
                        .join(&plugin.record.marketplace)
                        .join(&plugin.record.name),
                )?;
                publish_tree(&plugin.source, &plugin.record.root)?;
                published.push(plugin.record.root.clone());
                if plugin.id == id && update {
                    let index = registry
                        .installed
                        .iter()
                        .position(|p| self.applies(p) && Self::matches(p, &id, scope))
                        .context("installed root disappeared")?;
                    registry.installed[index] = plugin.record;
                } else {
                    registry.installed.push(plugin.record);
                }
            }
            if root.enabled {
                self.enable_dependency_closure(&mut registry, &root)?;
            }
            self.save(&registry)
        })();
        if let Err(mut error) = result {
            for path in published {
                if let Err(cleanup) = fs::remove_dir_all(&path) {
                    error = error.context(format!(
                        "could not remove abandoned plugin payload {}: {cleanup}",
                        path.display()
                    ));
                }
            }
            return Err(error);
        }
        Ok(root)
    }
}

fn installed_dependencies(record: &InstalledPlugin, base: &Path) -> Result<Vec<String>> {
    let base = base.canonicalize()?;
    let root = record.root.canonicalize()?;
    ensure!(
        root.starts_with(base),
        "dependency {}@{} escapes managed cache",
        record.name,
        record.marketplace
    );
    let manifest = PluginManager::validate(&root)?;
    ensure!(
        manifest.name == record.name,
        "dependency {}@{} manifest identity mismatch",
        record.name,
        record.marketplace
    );
    parse(manifest.extra.get("dependencies"))
}

pub(super) fn effective_records(
    records: Vec<InstalledPlugin>,
) -> BTreeMap<(String, String), InstalledPlugin> {
    let rank = |scope| match scope {
        PluginScope::User => 0,
        PluginScope::Project => 1,
        PluginScope::Local => 2,
    };
    let mut chosen: BTreeMap<(String, String), InstalledPlugin> = BTreeMap::new();
    for record in records {
        let key = (record.name.clone(), record.marketplace.clone());
        if chosen
            .get(&key)
            .is_none_or(|old| rank(record.scope) > rank(old.scope))
        {
            chosen.insert(key, record);
        }
    }
    chosen
}

pub(super) fn satisfied(
    records: &BTreeMap<(String, String), InstalledPlugin>,
    base: &Path,
) -> (HashSet<(String, String)>, Vec<String>) {
    fn check(
        key: &(String, String),
        records: &BTreeMap<(String, String), InstalledPlugin>,
        base: &Path,
        visiting: &mut Vec<(String, String)>,
        done: &mut BTreeMap<(String, String), std::result::Result<(), String>>,
    ) -> std::result::Result<(), String> {
        if let Some(result) = done.get(key) {
            return result.clone();
        }
        let id = format!("{}@{}", key.0, key.1);
        if visiting.contains(key) {
            return Err(format!("dependency cycle at {id}"));
        }
        if visiting.len() >= MAX_DEPTH || done.len() + visiting.len() >= MAX_NODES {
            return Err("dependency graph exceeds runtime size/depth limit".to_owned());
        }
        let record = records
            .get(key)
            .ok_or_else(|| format!("dependency {id} is not installed"))?;
        if !record.enabled {
            return Err(format!(
                "dependency {id} is disabled; enable dependency first"
            ));
        }
        if records.values().filter(|p| p.name == key.0).count() != 1 {
            return Err(format!(
                "dependency {id} has conflicting marketplace records"
            ));
        }
        let result = installed_dependencies(record, base)
            .map_err(|error| format!("dependency {id} is invalid: {error}"));
        let result = match result {
            Err(error) => Err(error),
            Ok(deps) => {
                visiting.push(key.clone());
                let result = deps.iter().try_for_each(|dependency| {
                    let dependency = qualified(dependency, &key.1);
                    let (name, market) = dependency
                        .split_once('@')
                        .ok_or_else(|| "dependency must have a marketplace".to_owned())?;
                    check(
                        &(name.to_owned(), market.to_owned()),
                        records,
                        base,
                        visiting,
                        done,
                    )
                });
                visiting.pop();
                result
            }
        };
        done.insert(key.clone(), result.clone());
        result
    }
    let mut allowed = HashSet::new();
    let mut diagnostics = Vec::new();
    for (key, record) in records {
        if !record.enabled {
            continue;
        }
        let mut done = BTreeMap::new();
        match check(key, records, base, &mut Vec::new(), &mut done) {
            Ok(()) => {
                allowed.insert(key.clone());
            }
            Err(error) => diagnostics.push(format!("plugin {}@{} inactive: {error}", key.0, key.1)),
        }
    }
    (allowed, diagnostics)
}
