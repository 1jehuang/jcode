//! D2 adapters to the existing PNG cache, image placeholders, and diagram registry.

#[cfg(feature = "renderer")]
use super::d2::compile_d2_cancellable;
use super::d2::{D2Options, resolve_d2_binary, validate_d2_source};
use super::{CachedDiagram, RenderProfile, RenderResult};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, mpsc};

const MAX_PENDING: usize = 32;
const MAX_SOURCE_RECORDS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramLanguage {
    Mermaid,
    D2,
}
impl DiagramLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mermaid => "Mermaid",
            Self::D2 => "D2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagramSource {
    pub language: DiagramLanguage,
    pub source: String,
}

#[derive(Clone)]
struct Settings {
    enabled: bool,
    #[cfg_attr(not(feature = "renderer"), allow(dead_code))]
    options: D2Options,
    identity: u64,
}
impl Settings {
    fn new(enabled: bool, mut options: D2Options) -> Self {
        let mut identity = std::collections::hash_map::DefaultHasher::new();
        if enabled
            && cfg!(feature = "renderer")
            && let Ok(binary) = resolve_d2_binary(&options.binary)
        {
            options.binary = binary;
            if let Ok(metadata) = options.binary.metadata() {
                metadata.len().hash(&mut identity);
                metadata.modified().ok().hash(&mut identity);
            }
        }
        options.hash(&mut identity);
        Self {
            enabled,
            options,
            identity: identity.finish(),
        }
    }
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();
static SOURCES: LazyLock<Mutex<HashMap<u64, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ERRORS: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
type RequestKey = (u64, u32);
struct Pending {
    cancel: Arc<AtomicBool>,
}
static PENDING: LazyLock<Mutex<HashMap<RequestKey, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static WORKER: OnceLock<Result<mpsc::SyncSender<Task>, String>> = OnceLock::new();
#[cfg(feature = "renderer")]
static RENDER_LOCK: Mutex<()> = Mutex::new(());
struct Task {
    key: RequestKey,
    source: String,
    settings: Settings,
    cancel: Arc<AtomicBool>,
}

/// First call wins. Render options are intentionally a TUI-client startup snapshot.
pub fn configure_d2(enabled: bool, options: D2Options) {
    let _ = SETTINGS.set(Settings::new(enabled, options));
}
fn settings() -> &'static Settings {
    SETTINGS.get_or_init(|| Settings::new(true, D2Options::default()))
}
pub fn d2_rendering_enabled() -> bool {
    cfg!(feature = "renderer") && settings().enabled
}
pub fn is_d2_lang(language: &str) -> bool {
    language
        .split_whitespace()
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case("d2"))
}

pub fn diagram_source_for_hash(hash: u64) -> Option<DiagramSource> {
    if let Some(source) = SOURCES
        .lock()
        .ok()
        .and_then(|sources| sources.get(&hash).cloned())
    {
        return Some(DiagramSource {
            language: DiagramLanguage::D2,
            source,
        });
    }
    super::mermaid_source_for_hash(hash).map(|source| DiagramSource {
        language: DiagramLanguage::Mermaid,
        source,
    })
}

pub(crate) fn clear_d2_state() {
    if let Ok(mut pending) = PENDING.lock() {
        for request in pending.values() {
            request.cancel.store(true, Ordering::Release);
        }
        pending.clear();
    }
    if let Ok(mut errors) = ERRORS.lock() {
        errors.clear();
    }
    if let Ok(mut sources) = SOURCES.lock() {
        sources.clear();
    }
}

fn bounded_insert(map: &mut HashMap<u64, String>, key: u64, value: String) {
    if !map.contains_key(&key)
        && map.len() >= MAX_SOURCE_RECORDS
        && let Some(oldest) = map.keys().next().copied()
    {
        map.remove(&oldest);
    }
    map.insert(key, value);
}

fn source_hash(source: &str, settings: &Settings) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "jcode-d2-pipeline-v1-self-contained".hash(&mut hasher);
    settings.identity.hash(&mut hasher);
    source.hash(&mut hasher);
    hasher.finish()
}
fn target_width(terminal_width: Option<u16>) -> u32 {
    let cell_width = super::get_font_size().map(|(width, _)| width).unwrap_or(8);
    super::svg::normalize_render_target_width(
        f64::from(terminal_width.unwrap_or(100)) * f64::from(cell_width) * 2.0,
    )
}
fn cached_result(hash: u64, width: u32, register: bool) -> Option<RenderResult> {
    let cached = super::cache_render::get_cached_diagram_for_profile(
        hash,
        Some(width),
        RenderProfile::default(),
    )?;
    if register {
        super::register_active_diagram(hash, cached.width, cached.height, Some("D2".into()));
    }
    Some(RenderResult::Image {
        hash,
        path: cached.path,
        width: cached.width,
        height: cached.height,
    })
}

/// Uses the real production compiler/raster/cache path, useful for local probes.
pub fn render_d2_sized_with_options(
    source: &str,
    terminal_width: Option<u16>,
    options: D2Options,
    register: bool,
) -> RenderResult {
    let settings = Settings::new(true, options);
    let hash = source_hash(source, &settings);
    if let Err(error) = validate_d2_source(source) {
        return RenderResult::Error(error);
    }
    if let Ok(mut sources) = SOURCES.lock() {
        bounded_insert(&mut sources, hash, source.to_owned());
    }
    let width = target_width(terminal_width);
    if let Some(result) = cached_result(hash, width, register) {
        return result;
    }
    match render_ready(
        source,
        &settings,
        hash,
        width,
        Arc::new(AtomicBool::new(false)),
    ) {
        Ok(cached) => {
            if register {
                super::register_active_diagram(
                    hash,
                    cached.width,
                    cached.height,
                    Some("D2".into()),
                );
            }
            RenderResult::Image {
                hash,
                path: cached.path,
                width: cached.width,
                height: cached.height,
            }
        }
        Err(error) => RenderResult::Error(error),
    }
}

pub fn render_d2_deferred_with_registration(
    source: &str,
    terminal_width: Option<u16>,
    register: bool,
    _stream_scope: Option<u64>,
) -> Option<RenderResult> {
    if !d2_rendering_enabled() {
        return Some(RenderResult::Error(
            "D2 rendering is disabled in this client".into(),
        ));
    }
    if let Err(error) = validate_d2_source(source) {
        return Some(RenderResult::Error(error));
    }
    let settings = settings().clone();
    let hash = source_hash(source, &settings);
    let width = target_width(terminal_width);
    if let Ok(mut sources) = SOURCES.lock() {
        bounded_insert(&mut sources, hash, source.to_owned());
    }
    if let Some(result) = cached_result(hash, width, register) {
        return Some(result);
    }
    if let Some(error) = ERRORS
        .lock()
        .ok()
        .and_then(|errors| errors.get(&hash).cloned())
    {
        return Some(RenderResult::Error(error));
    }
    let key = (hash, width);
    let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    // Closed fences are immutable while surrounding prose streams. A caller's
    // block ordinal is not a global stream identity, so it must never cancel
    // a different artifact from another renderer. Deduplicate only by key.
    if pending.contains_key(&key) {
        return None;
    }
    if pending.len() >= MAX_PENDING {
        return Some(RenderResult::Error(
            "D2 render queue is full; showing source".into(),
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    pending.insert(
        key,
        Pending {
            cancel: cancel.clone(),
        },
    );
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Task>(MAX_PENDING);
        std::thread::Builder::new()
            .name("jcode-d2-render".into())
            .spawn(move || {
                for task in rx {
                    if task.cancel.load(Ordering::Acquire) {
                        continue;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        render_ready(
                            &task.source,
                            &task.settings,
                            task.key.0,
                            task.key.1,
                            task.cancel.clone(),
                        )
                    }))
                    .unwrap_or_else(|_| Err("D2 renderer failed; showing source".into()));
                    let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
                    let current = pending
                        .get(&task.key)
                        .is_some_and(|entry| Arc::ptr_eq(&entry.cancel, &task.cancel));
                    if current {
                        pending.remove(&task.key);
                        if !task.cancel.load(Ordering::Acquire) {
                            if let Err(error) = result
                                && let Ok(mut errors) = ERRORS.lock()
                            {
                                bounded_insert(&mut errors, task.key.0, error);
                            }
                            drop(pending);
                            super::notify_deferred_render_completed();
                        }
                    }
                }
            })
            .map(|_| tx)
            .map_err(|error| format!("Start D2 render worker: {error}"))
    });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            pending.remove(&key);
            return Some(RenderResult::Error(error.clone()));
        }
    };
    let task = Task {
        key,
        source: source.to_owned(),
        settings,
        cancel,
    };
    if let Err(error) = worker.try_send(task) {
        pending.remove(&key);
        return Some(RenderResult::Error(format!(
            "D2 render queue unavailable: {error}"
        )));
    }
    None
}

#[cfg(feature = "renderer")]
fn render_ready(
    source: &str,
    settings: &Settings,
    hash: u64,
    width: u32,
    cancel: Arc<AtomicBool>,
) -> Result<CachedDiagram, String> {
    let _render_guard = RENDER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if cancel.load(Ordering::Acquire) {
        return Err("D2 compilation cancelled".into());
    }
    if let Some(cached) = super::cache_render::get_cached_diagram_for_profile(
        hash,
        Some(width),
        RenderProfile::default(),
    ) {
        return Ok(cached);
    }
    let svg = compile_d2_cancellable(source, &settings.options, cancel.clone())?;
    let cache_path = {
        let cache = super::RENDER_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        cache.cache_dir.join(format!("{hash:016x}_w{width}.png"))
    };
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Create D2 cache: {e}"))?;
    }
    let temporary = cache_path.with_extension(format!("{}.tmp.png", std::process::id()));
    let dimensions = super::svg::write_d2_png(&svg, &temporary, width);
    if cancel.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(&temporary);
        return Err("D2 compilation cancelled".into());
    }
    let (width, height) = match dimensions {
        Ok(dimensions) => dimensions,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.to_string());
        }
    };
    std::fs::rename(&temporary, &cache_path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("Publish D2 PNG: {e}")
    })?;
    let cached = CachedDiagram {
        path: cache_path,
        width,
        height,
    };
    super::RENDER_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(hash, RenderProfile::default(), cached.clone());
    super::cache_render::invalidate_cached_image(hash);
    Ok(cached)
}
#[cfg(not(feature = "renderer"))]
fn render_ready(
    _: &str,
    _: &Settings,
    _: u64,
    _: u32,
    _: Arc<AtomicBool>,
) -> Result<CachedDiagram, String> {
    Err("D2 rendering is disabled in this build".into())
}

#[cfg(test)]
#[path = "diagram_engine_tests.rs"]
mod tests;
