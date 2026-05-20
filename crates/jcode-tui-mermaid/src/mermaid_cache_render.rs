use super::*;

/// Maximum in-memory RENDER_CACHE entries (metadata only, not images).
pub(super) const RENDER_CACHE_MAX: usize = 64;
/// Reuse a cached PNG only if it's at least this fraction of requested width.
/// This avoids visibly blurry upscaling after terminal/pane resizes.
pub(super) const CACHE_WIDTH_MATCH_PERCENT: u32 = 85;
/// Quantize requested Mermaid render widths so tiny pane-width changes, like a
/// 1-cell scrollbar reservation, reuse the same cold render/cache entry.
pub(super) const RENDER_WIDTH_BUCKET_CELLS: u32 = 4;

/// Mermaid rendering cache
pub(super) struct MermaidCache {
    /// Map from content hash to rendered PNG info
    pub(super) entries: HashMap<(u64, RenderProfile), CachedDiagram>,
    /// Insertion order for LRU eviction
    pub(super) order: VecDeque<(u64, RenderProfile)>,
    /// Cache directory
    pub(super) cache_dir: PathBuf,
}

#[derive(Clone)]
pub(super) struct CachedDiagram {
    pub(super) path: PathBuf,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl MermaidCache {
    pub(super) fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("jcode")
            .join("mermaid");

        let _ = fs::create_dir_all(&cache_dir);

        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            cache_dir,
        }
    }

    fn touch(&mut self, key: (u64, RenderProfile)) {
        if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key);
    }

    pub(super) fn get(
        &mut self,
        hash: u64,
        min_width: Option<u32>,
        profile: Option<RenderProfile>,
    ) -> Option<CachedDiagram> {
        if let Some(profile) = profile {
            return self.get_exact_profile(hash, min_width, profile);
        }

        if let Some((key, existing)) = self.order.iter().rev().find_map(|key| {
            let (entry_hash, _) = *key;
            let existing = self.entries.get(key)?;
            if entry_hash == hash && cached_width_satisfies(existing.width, min_width) {
                Some((*key, existing.clone()))
            } else {
                None
            }
        }) {
            if existing.path.exists() {
                self.touch(key);
                return Some(existing);
            }
            self.entries.remove(&key);
            if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
                self.order.remove(pos);
            }
        }

        if let Some(found) = self.discover_on_disk(hash, min_width, None) {
            self.insert(hash, RenderProfile::default(), found.clone());
            return Some(found);
        }

        None
    }

    fn get_exact_profile(
        &mut self,
        hash: u64,
        min_width: Option<u32>,
        profile: RenderProfile,
    ) -> Option<CachedDiagram> {
        let key = (hash, profile);
        if let Some(existing) = self.entries.get(&key).cloned() {
            if existing.path.exists() && cached_width_satisfies(existing.width, min_width) {
                self.touch(key);
                return Some(existing);
            }
            self.entries.remove(&key);
            if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
                self.order.remove(pos);
            }
        }

        if let Some(found) = self.discover_on_disk(hash, min_width, Some(profile)) {
            self.insert(hash, profile, found.clone());
            return Some(found);
        }

        None
    }

    pub(super) fn insert(&mut self, hash: u64, profile: RenderProfile, diagram: CachedDiagram) {
        let key = (hash, profile);
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(key) {
            entry.insert(diagram);
            self.touch(key);
        } else {
            self.entries.insert(key, diagram);
            self.order.push_back(key);
            while self.order.len() > RENDER_CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    #[cfg(feature = "renderer")]
    pub(super) fn cache_path(
        &self,
        hash: u64,
        target_width: u32,
        profile: RenderProfile,
    ) -> PathBuf {
        // Include target width in filename for size-specific caching
        let suffix = profile.cache_suffix().unwrap_or_default();
        self.cache_dir
            .join(format!("{:016x}_w{}{}.png", hash, target_width, suffix))
    }

    pub(super) fn discover_on_disk(
        &self,
        hash: u64,
        min_width: Option<u32>,
        profile: Option<RenderProfile>,
    ) -> Option<CachedDiagram> {
        let mut candidates: Vec<(PathBuf, u32, RenderProfile)> = Vec::new();
        let entries = fs::read_dir(&self.cache_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let Some((file_hash, width_hint, file_profile)) = parse_cache_filename(&path) else {
                continue;
            };
            if file_hash == hash && profile.is_none_or(|profile| profile == file_profile) {
                candidates.push((path, width_hint, file_profile));
            }
        }
        if candidates.is_empty() {
            return None;
        }

        let selected = if let Some(min_w) = min_width {
            if let Some(candidate) = candidates
                .iter()
                .filter(|(_, w, _)| cached_width_satisfies(*w, Some(min_w)))
                .min_by_key(|(_, w, _)| *w)
            {
                candidate.clone()
            } else {
                return None;
            }
        } else {
            candidates
                .iter()
                .max_by_key(|(_, w, _)| *w)
                .cloned()
                .unwrap_or_else(|| candidates[0].clone())
        };

        let (path, width_hint, _) = selected;
        let (width, height) = get_png_dimensions(&path).unwrap_or((width_hint, width_hint));
        Some(CachedDiagram {
            path,
            width,
            height,
        })
    }
}

pub(super) fn cached_width_satisfies(width: u32, min_width: Option<u32>) -> bool {
    let Some(min_width) = min_width else {
        return true;
    };
    if min_width == 0 {
        return true;
    }
    width.saturating_mul(100) >= min_width.saturating_mul(CACHE_WIDTH_MATCH_PERCENT)
}

pub(super) fn parse_cache_filename(path: &Path) -> Option<(u64, u32, RenderProfile)> {
    let stem = path.file_stem()?.to_str()?;
    let (hash_hex, width_part) = stem.split_once("_w")?;
    let hash = u64::from_str_radix(hash_hex, 16).ok()?;
    let (width_text, profile) = if let Some((width, aspect)) = width_part.split_once("_a") {
        let aspect = aspect.parse::<u16>().ok()?;
        (
            width,
            RenderProfile {
                preferred_aspect_per_mille: Some(aspect),
            },
        )
    } else {
        (width_part, RenderProfile::default())
    };
    let width = width_text.parse::<u32>().ok()?;
    Some((hash, width, profile))
}

pub(super) fn get_cached_diagram(hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
    let profile = current_render_profile();
    let mut cache = RENDER_CACHE.lock().ok()?;
    if let Some(diagram) = cache.get(hash, min_width, Some(profile)) {
        return Some(diagram);
    }
    cache.get(hash, min_width, None)
}

fn get_cached_diagram_for_profile(
    hash: u64,
    min_width: Option<u32>,
    profile: RenderProfile,
) -> Option<CachedDiagram> {
    let mut cache = RENDER_CACHE.lock().ok()?;
    cache.get(hash, min_width, Some(profile))
}

pub fn get_cached_path(hash: u64) -> Option<PathBuf> {
    get_cached_diagram(hash, None).map(|c| c.path)
}

#[cfg(feature = "renderer")]
fn invalidate_cached_image(hash: u64) {
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.remove(&hash);
    }
    if let Ok(mut kitty) = KITTY_VIEWPORT_STATE.lock() {
        kitty.remove(&hash);
    }
    if let Ok(mut source) = SOURCE_CACHE.lock() {
        source.remove(hash);
    }
}

/// Result of attempting to render a mermaid diagram
pub enum RenderResult {
    /// Successfully rendered to image - includes content hash for state lookup
    Image {
        hash: u64,
        path: PathBuf,
        width: u32,
        height: u32,
    },
    /// Error during rendering
    Error(String),
}

/// Check if a code block language is mermaid
pub fn is_mermaid_lang(lang: &str) -> bool {
    let lang_lower = lang.to_lowercase();
    lang_lower == "mermaid" || lang_lower.starts_with("mermaid")
}

/// Maximum allowed nodes in a diagram (prevents OOM on complex diagrams)
const MAX_NODES: usize = 100;
/// Maximum allowed edges in a diagram
const MAX_EDGES: usize = 200;

/// Count nodes and edges in mermaid content (rough estimate)
pub(super) fn estimate_diagram_size(content: &str) -> (usize, usize) {
    svg::estimate_diagram_size(content)
}

/// Calculate optimal PNG dimensions based on terminal and diagram complexity
pub(super) fn calculate_render_size(
    node_count: usize,
    edge_count: usize,
    terminal_width: Option<u16>,
) -> (f64, f64) {
    let (width, height) = svg::calculate_render_size(node_count, edge_count, terminal_width);
    if let Some(aspect) = current_render_profile().preferred_aspect_ratio() {
        let profile_height = (width / aspect as f64).clamp(300.0, DEFAULT_RENDER_HEIGHT as f64);
        (width, profile_height)
    } else {
        (width, height)
    }
}

#[cfg(feature = "renderer")]
fn svg_dimension_to_u32(value: f32) -> u32 {
    if value.is_finite() && value > 0.0 {
        value.round().clamp(1.0, u32::MAX as f32) as u32
    } else {
        1
    }
}

#[cfg(feature = "renderer")]
fn write_output_png_cached_fonts(
    svg: &str,
    output: &Path,
    render_cfg: &RenderConfig,
    theme: &Theme,
) -> anyhow::Result<()> {
    svg::write_output_png_cached_fonts(svg, output, render_cfg, theme)
}

/// Render a mermaid code block to PNG (cached)
/// Now accepts optional terminal_width for adaptive sizing
pub fn render_mermaid(content: &str) -> RenderResult {
    render_mermaid_sized(content, None)
}

/// Render with explicit terminal width for adaptive sizing
pub fn render_mermaid_sized(content: &str, terminal_width: Option<u16>) -> RenderResult {
    render_mermaid_sized_internal(content, terminal_width, true)
}

/// Render without registering the diagram in ACTIVE_DIAGRAMS.
/// Useful for internal widget visuals that should not appear in the
/// user-visible diagram pane.
pub fn render_mermaid_untracked(content: &str, terminal_width: Option<u16>) -> RenderResult {
    render_mermaid_sized_internal(content, terminal_width, false)
}

pub(super) fn bump_deferred_render_epoch() {
    DEFERRED_RENDER_EPOCH.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.deferred_epoch_bumps += 1;
    }
}

pub fn deferred_render_epoch() -> u64 {
    DEFERRED_RENDER_EPOCH.load(Ordering::Relaxed)
}

fn deferred_render_sender() -> &'static mpsc::Sender<DeferredRenderTask> {
    DEFERRED_RENDER_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<DeferredRenderTask>();
        if let Err(err) = std::thread::Builder::new()
            .name("jcode-mermaid-deferred".to_string())
            .spawn(move || deferred_render_worker(rx))
        {
            crate::log_warn(&format!(
                "Failed to spawn mermaid deferred worker, falling back to synchronous rendering: {}",
                err
            ));
        }
        tx
    })
}

fn deferred_render_worker(rx: mpsc::Receiver<DeferredRenderTask>) {
    for task in rx {
        let register_active = match PENDING_RENDER_REQUESTS.lock() {
            Ok(pending) => pending
                .get(&task.render_key)
                .map(|request| request.register_active),
            Err(poisoned) => poisoned
                .into_inner()
                .get(&task.render_key)
                .map(|request| request.register_active),
        };

        let Some(register_active) = register_active else {
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.deferred_worker_skips += 1;
            }
            continue;
        };

        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.deferred_worker_renders += 1;
        }

        let profile = task.render_key.2;
        let _ = with_preferred_aspect_ratio(profile.preferred_aspect_ratio(), || {
            render_mermaid_sized_internal(&task.content, task.terminal_width, register_active)
        });

        if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
            pending.remove(&task.render_key);
        }
        bump_deferred_render_epoch();
        crate::notify_render_completed();
    }
}

pub(crate) fn is_likely_stream_update(previous: &str, next: &str) -> bool {
    let previous = previous.trim_end();
    let next = next.trim_end();
    if previous == next || previous.len().min(next.len()) < 16 {
        return false;
    }
    next.starts_with(previous) || previous.starts_with(next)
}

/// Streaming-friendly Mermaid rendering.
///
/// If the diagram is already cached, returns it immediately. Otherwise this
/// queues the heavy render work onto a background thread and returns `None`
/// so the caller can keep the UI responsive with a lightweight placeholder.
pub fn render_mermaid_deferred(content: &str, terminal_width: Option<u16>) -> Option<RenderResult> {
    render_mermaid_deferred_with_registration(content, terminal_width, false)
}

pub fn render_mermaid_deferred_with_registration(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
) -> Option<RenderResult> {
    render_mermaid_deferred_inner(content, terminal_width, register_active, None)
}

pub fn render_mermaid_deferred_with_stream_scope(
    content: &str,
    terminal_width: Option<u16>,
    stream_scope: u64,
) -> Option<RenderResult> {
    render_mermaid_deferred_inner(content, terminal_width, false, Some(stream_scope))
}

fn render_mermaid_deferred_inner(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
    stream_scope: Option<u64>,
) -> Option<RenderResult> {
    let hash = hash_content(content);
    let (node_count, edge_count) = estimate_diagram_size(content);

    if node_count > MAX_NODES || edge_count > MAX_EDGES {
        return Some(RenderResult::Error(format!(
            "Diagram too complex ({} nodes, {} edges). Max: {} nodes, {} edges.",
            node_count, edge_count, MAX_NODES, MAX_EDGES
        )));
    }

    let (target_width, _) = calculate_render_size(node_count, edge_count, terminal_width);
    let target_width_u32 = target_width as u32;
    let render_profile = current_render_profile();

    if let Some(cached) =
        get_cached_diagram_for_profile(hash, Some(target_width_u32), render_profile)
    {
        if register_active {
            register_active_diagram(hash, cached.width, cached.height, None);
        }
        return Some(RenderResult::Image {
            hash,
            path: cached.path,
            width: cached.width,
            height: cached.height,
        });
    }

    if let Some(err) = RENDER_ERRORS
        .lock()
        .ok()
        .and_then(|errors| errors.get(&hash).cloned())
    {
        return Some(RenderResult::Error(err));
    }

    let render_key = (hash, target_width_u32, render_profile);
    let should_enqueue =
        match PENDING_RENDER_REQUESTS.lock() {
            Ok(mut pending) => {
                let mut superseded = 0u64;
                pending.retain(|(_, pending_width, pending_profile), request| {
                    let same_stream_scope =
                        request.stream_scope.is_some() && request.stream_scope == stream_scope;
                    let same_profile = *pending_profile == render_profile;
                    let same_terminal_width = request.terminal_width == terminal_width;
                    let compatible_width =
                        cached_width_satisfies(*pending_width, Some(target_width_u32))
                            || cached_width_satisfies(target_width_u32, Some(*pending_width));
                    let supersede = same_stream_scope
                        && same_profile
                        && same_terminal_width
                        && compatible_width
                        && is_likely_stream_update(&request.content, content);
                    if supersede {
                        superseded = superseded.saturating_add(1);
                    }
                    !supersede
                });
                if superseded > 0
                    && let Ok(mut state) = MERMAID_DEBUG.lock()
                {
                    state.stats.deferred_superseded =
                        state.stats.deferred_superseded.saturating_add(superseded);
                }

                if let Some((_, existing_request)) = pending.iter_mut().find(
                    |((pending_hash, pending_width, pending_profile), _)| {
                        *pending_hash == hash
                            && *pending_profile == render_profile
                            && cached_width_satisfies(*pending_width, Some(target_width_u32))
                    },
                ) {
                    if register_active {
                        existing_request.register_active = true;
                    }
                    if let Ok(mut state) = MERMAID_DEBUG.lock() {
                        state.stats.deferred_deduped += 1;
                    }
                    false
                } else {
                    match pending.entry(render_key) {
                        Entry::Occupied(mut occupied) => {
                            if register_active {
                                occupied.get_mut().register_active = true;
                            }
                            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                                state.stats.deferred_deduped += 1;
                            }
                            false
                        }
                        Entry::Vacant(vacant) => {
                            vacant.insert(PendingDeferredRender {
                                register_active,
                                terminal_width,
                                content: content.to_string(),
                                stream_scope,
                            });
                            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                                state.stats.deferred_enqueued += 1;
                            }
                            true
                        }
                    }
                }
            }
            Err(_) => {
                return Some(render_mermaid_sized_internal(
                    content,
                    terminal_width,
                    register_active,
                ));
            }
        };

    if should_enqueue {
        let task = DeferredRenderTask {
            content: content.to_string(),
            terminal_width,
            render_key,
        };
        if deferred_render_sender().send(task).is_err() {
            if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
                pending.remove(&render_key);
            }
            return Some(render_mermaid_sized_internal(
                content,
                terminal_width,
                register_active,
            ));
        }
    }

    None
}

fn render_mermaid_sized_internal(
    content: &str,
    terminal_width: Option<u16>,
    register_active: bool,
) -> RenderResult {
    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.total_requests += 1;
        state.stats.last_content_len = Some(content.len());
        state.stats.last_error = None;
        state.stats.last_parse_ms = None;
        state.stats.last_layout_ms = None;
        state.stats.last_svg_ms = None;
        state.stats.last_png_ms = None;
    }

    // Calculate content hash for caching
    let hash = hash_content(content);
    let render_profile = current_render_profile();

    // Estimate complexity for sizing
    let (node_count, edge_count) = estimate_diagram_size(content);
    #[cfg(feature = "renderer")]
    let complexity = node_count + edge_count;

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_nodes = Some(node_count);
        state.stats.last_edges = Some(edge_count);
    }

    // Check complexity limits
    if node_count > MAX_NODES || edge_count > MAX_EDGES {
        let msg = format!(
            "Diagram too complex ({} nodes, {} edges). Max: {} nodes, {} edges.",
            node_count, edge_count, MAX_NODES, MAX_EDGES
        );
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.render_errors += 1;
            state.stats.last_error = Some(msg.clone());
        }
        return RenderResult::Error(msg);
    }

    // Calculate target size
    let (target_width, target_height) =
        calculate_render_size(node_count, edge_count, terminal_width);
    let target_width_u32 = target_width as u32;
    let target_height_u32 = target_height as u32;

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.last_target_width = Some(target_width_u32);
        state.stats.last_target_height = Some(target_height_u32);
    }

    // Check cache (memory + on-disk fallback, width-aware).
    if let Some(cached) =
        get_cached_diagram_for_profile(hash, Some(target_width_u32), render_profile)
    {
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.cache_hits += 1;
            state.stats.last_hash = Some(format!("{:016x}", hash));
        }
        if register_active {
            // Register as active diagram (for pinned widget display)
            register_active_diagram(hash, cached.width, cached.height, None);
        }
        return RenderResult::Image {
            hash,
            path: cached.path,
            width: cached.width,
            height: cached.height,
        };
    }

    if let Ok(mut state) = MERMAID_DEBUG.lock() {
        state.stats.cache_misses += 1;
        state.stats.last_hash = Some(format!("{:016x}", hash));
    }

    #[cfg(not(feature = "renderer"))]
    {
        let msg = "Mermaid rendering is disabled in this build".to_string();
        if let Ok(mut errors) = RENDER_ERRORS.lock() {
            errors.insert(hash, msg.clone());
        }
        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.render_errors += 1;
            state.stats.last_error = Some(msg.clone());
        }
        RenderResult::Error(msg)
    }

    #[cfg(feature = "renderer")]
    {
        // Get cache path
        let png_path = {
            let cache = RENDER_CACHE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.cache_path(hash, target_width_u32, render_profile)
        };
        let png_path_clone = png_path.clone();

        let _render_guard = RENDER_WORK_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Re-check cache after taking the render lock so a background worker that
        // just finished can satisfy this request without doing duplicate work.
        if let Some(cached) =
            get_cached_diagram_for_profile(hash, Some(target_width_u32), render_profile)
        {
            if let Ok(mut errors) = RENDER_ERRORS.lock() {
                errors.remove(&hash);
            }
            if let Ok(mut state) = MERMAID_DEBUG.lock() {
                state.stats.cache_hits += 1;
                state.stats.last_hash = Some(format!("{:016x}", hash));
            }
            if register_active {
                register_active_diagram(hash, cached.width, cached.height, None);
            }
            return RenderResult::Image {
                hash,
                path: cached.path,
                width: cached.width,
                height: cached.height,
            };
        }

        // Wrap mermaid library calls in catch_unwind for defense-in-depth
        let content_owned = content.to_string();

        let prev_hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {
            // Silently ignore panics from mermaid renderer
        }));

        let render_start = Instant::now();
        let render_result = panic::catch_unwind(move || -> Result<RenderStageBreakdown, String> {
            let parse_start = Instant::now();
            // Parse mermaid
            let parsed =
                parse_mermaid(&content_owned).map_err(|e| format!("Parse error: {}", e))?;
            let parse_ms = parse_start.elapsed().as_secs_f32() * 1000.0;

            // Configure theme for terminal (dark background friendly)
            let theme = terminal_theme();

            // Adaptive spacing based on complexity
            let spacing_factor = if complexity > 30 { 1.2 } else { 1.0 };
            let layout_config = LayoutConfig {
                node_spacing: 80.0 * spacing_factor,
                rank_spacing: 80.0 * spacing_factor,
                node_padding_x: 40.0,
                node_padding_y: 20.0,
                preferred_aspect_ratio: render_profile.preferred_aspect_ratio(),
                ..Default::default()
            };

            let layout_start = Instant::now();
            // Compute layout
            let layout = compute_layout(&parsed.graph, &theme, &layout_config);
            let layout_ms = layout_start.elapsed().as_secs_f32() * 1000.0;

            let svg_start = Instant::now();
            let output_dimensions = Some((target_width as f32, target_height as f32));
            // Render and collect size metadata. With the mmdr size API enabled this
            // comes directly from the renderer; the default compatibility path keeps
            // the old SVG retargeting behavior until the dependency is updated.
            let (svg, dimensions) =
                render_svg_for_png(&layout, &theme, &layout_config, output_dimensions);
            let svg_ms = svg_start.elapsed().as_secs_f32() * 1000.0;

            // Convert SVG to PNG with adaptive dimensions
            let render_config = RenderConfig {
                width: dimensions.width,
                height: dimensions.height,
                background: theme.background.clone(),
            };

            // Ensure parent directory exists
            if let Some(parent) = png_path_clone.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create cache directory: {}", e))?;
            }

            let png_start = Instant::now();
            write_output_png_cached_fonts(&svg, &png_path_clone, &render_config, &theme)
                .map_err(|e| format!("Render error: {}", e))?;
            let png_ms = png_start.elapsed().as_secs_f32() * 1000.0;

            Ok(RenderStageBreakdown {
                parse_ms,
                layout_ms,
                svg_ms,
                png_ms,
                measured_width: svg_dimension_to_u32(dimensions.width),
                measured_height: svg_dimension_to_u32(dimensions.height),
                viewbox_width: svg_dimension_to_u32(dimensions.viewbox_width),
                viewbox_height: svg_dimension_to_u32(dimensions.viewbox_height),
            })
        });

        // Restore the original panic hook
        panic::set_hook(prev_hook);

        // Handle the result
        let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
        let stage_breakdown = match render_result {
            Ok(Ok(stage_breakdown)) => {
                if let Ok(mut errors) = RENDER_ERRORS.lock() {
                    errors.remove(&hash);
                }
                if let Ok(mut state) = MERMAID_DEBUG.lock() {
                    state.stats.render_success += 1;
                    state.stats.last_render_ms = Some(render_ms);
                    state.stats.last_parse_ms = Some(stage_breakdown.parse_ms);
                    state.stats.last_layout_ms = Some(stage_breakdown.layout_ms);
                    state.stats.last_svg_ms = Some(stage_breakdown.svg_ms);
                    state.stats.last_png_ms = Some(stage_breakdown.png_ms);
                    state.stats.last_measured_width = Some(stage_breakdown.measured_width);
                    state.stats.last_measured_height = Some(stage_breakdown.measured_height);
                    state.stats.last_viewbox_width = Some(stage_breakdown.viewbox_width);
                    state.stats.last_viewbox_height = Some(stage_breakdown.viewbox_height);
                }
                stage_breakdown
            }
            Ok(Err(e)) => {
                if let Ok(mut errors) = RENDER_ERRORS.lock() {
                    errors.insert(hash, e.clone());
                }
                if let Ok(mut state) = MERMAID_DEBUG.lock() {
                    state.stats.render_errors += 1;
                    state.stats.last_render_ms = Some(render_ms);
                    state.stats.last_error = Some(e.clone());
                }
                return RenderResult::Error(e);
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic in mermaid renderer".to_string()
                };
                if let Ok(mut errors) = RENDER_ERRORS.lock() {
                    errors.insert(hash, format!("Renderer panic: {}", msg));
                }
                if let Ok(mut state) = MERMAID_DEBUG.lock() {
                    state.stats.render_errors += 1;
                    state.stats.last_render_ms = Some(render_ms);
                    state.stats.last_error = Some(format!("Renderer panic: {}", msg));
                }
                return RenderResult::Error(format!("Renderer panic: {}", msg));
            }
        };

        // Get actual dimensions from rendered PNG
        let (width, height) = get_png_dimensions(&png_path).unwrap_or((
            stage_breakdown.measured_width,
            stage_breakdown.measured_height,
        ));

        if let Ok(mut state) = MERMAID_DEBUG.lock() {
            state.stats.last_png_width = Some(width);
            state.stats.last_png_height = Some(height);
        }

        // Cache the result
        {
            let mut cache = RENDER_CACHE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.insert(
                hash,
                render_profile,
                CachedDiagram {
                    path: png_path.clone(),
                    width,
                    height,
                },
            );
        }
        // If we re-rendered at a new size/path, force widget state to reload.
        invalidate_cached_image(hash);

        if register_active {
            // Register this diagram as active for info widget display
            register_active_diagram(hash, width, height, None);
        }

        RenderResult::Image {
            hash,
            path: png_path,
            width,
            height,
        }
    }
}
