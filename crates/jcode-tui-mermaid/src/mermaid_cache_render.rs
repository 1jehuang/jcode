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
    pub(super) entries: HashMap<u64, CachedDiagram>,
    /// Insertion order for LRU eviction
    pub(super) order: VecDeque<u64>,
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

    fn touch(&mut self, hash: u64) {
        if let Some(pos) = self.order.iter().position(|h| *h == hash) {
            self.order.remove(pos);
        }
        self.order.push_back(hash);
    }

    pub(super) fn get(&mut self, hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
        if let Some(existing) = self.entries.get(&hash).cloned() {
            if existing.path.exists() && cached_width_satisfies(existing.width, min_width) {
                self.touch(hash);
                return Some(existing);
            }
            self.entries.remove(&hash);
            if let Some(pos) = self.order.iter().position(|h| *h == hash) {
                self.order.remove(pos);
            }
        }

        if let Some(found) = self.discover_on_disk(hash, min_width) {
            self.insert(hash, found.clone());
            return Some(found);
        }

        None
    }

    pub(super) fn insert(&mut self, hash: u64, diagram: CachedDiagram) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.entries.entry(hash) {
            entry.insert(diagram);
            self.touch(hash);
        } else {
            self.entries.insert(hash, diagram);
            self.order.push_back(hash);
            while self.order.len() > RENDER_CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    pub(super) fn cache_path(&self, hash: u64, target_width: u32) -> PathBuf {
        // Include target width in filename for size-specific caching
        self.cache_dir
            .join(format!("{:016x}_w{}.png", hash, target_width))
    }

    pub(super) fn discover_on_disk(
        &self,
        hash: u64,
        min_width: Option<u32>,
    ) -> Option<CachedDiagram> {
        let mut candidates: Vec<(PathBuf, u32)> = Vec::new();
        let entries = fs::read_dir(&self.cache_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let Some((file_hash, width_hint)) = parse_cache_filename(&path) else {
                continue;
            };
            if file_hash == hash {
                candidates.push((path, width_hint));
            }
        }
        if candidates.is_empty() {
            return None;
        }

        let selected = if let Some(min_w) = min_width {
            if let Some(candidate) = candidates
                .iter()
                .filter(|(_, w)| cached_width_satisfies(*w, Some(min_w)))
                .min_by_key(|(_, w)| *w)
            {
                candidate.clone()
            } else {
                return None;
            }
        } else {
            candidates
                .iter()
                .max_by_key(|(_, w)| *w)
                .cloned()
                .unwrap_or_else(|| candidates[0].clone())
        };

        let (path, width_hint) = selected;
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

pub(super) fn parse_cache_filename(path: &Path) -> Option<(u64, u32)> {
    let stem = path.file_stem()?.to_str()?;
    let (hash_hex, width_part) = stem.split_once("_w")?;
    let hash = u64::from_str_radix(hash_hex, 16).ok()?;
    let width = width_part.parse::<u32>().ok()?;
    Some((hash, width))
}

pub(super) fn get_cached_diagram(hash: u64, min_width: Option<u32>) -> Option<CachedDiagram> {
    let mut cache = RENDER_CACHE.lock().ok()?;
    cache.get(hash, min_width)
}

pub fn get_cached_path(hash: u64) -> Option<PathBuf> {
    get_cached_diagram(hash, None).map(|c| c.path)
}

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
    svg::calculate_render_size(node_count, edge_count, terminal_width)
}

pub(super) fn retarget_svg_for_png(svg: &str, target_width: f64, target_height: f64) -> String {
    svg::retarget_svg_for_png(svg, target_width, target_height)
}

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

        let _ = render_mermaid_sized_internal(&task.content, task.terminal_width, register_active);

        if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
            pending.remove(&task.render_key);
        }
        bump_deferred_render_epoch();
        crate::notify_render_completed();
    }
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

    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
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

    let render_key = (hash, target_width_u32);
    let should_enqueue = match PENDING_RENDER_REQUESTS.lock() {
        Ok(mut pending) => {
            if let Some((_, existing_request)) =
                pending
                    .iter_mut()
                    .find(|((pending_hash, pending_width), _)| {
                        *pending_hash == hash
                            && cached_width_satisfies(*pending_width, Some(target_width_u32))
                    })
            {
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
                        vacant.insert(PendingDeferredRender { register_active });
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

    // Estimate complexity for sizing
    let (node_count, edge_count) = estimate_diagram_size(content);
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
    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
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

    // Get cache path
    let png_path = {
        let cache = RENDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.cache_path(hash, target_width_u32)
    };
    let png_path_clone = png_path.clone();

    let _render_guard = RENDER_WORK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Re-check cache after taking the render lock so a background worker that
    // just finished can satisfy this request without doing duplicate work.
    if let Some(cached) = get_cached_diagram(hash, Some(target_width_u32)) {
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
        let parsed = parse_mermaid(&content_owned).map_err(|e| format!("Parse error: {}", e))?;
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
            ..Default::default()
        };

        let layout_start = Instant::now();
        // Compute layout
        let layout = compute_layout(&parsed.graph, &theme, &layout_config);
        let layout_ms = layout_start.elapsed().as_secs_f32() * 1000.0;

        let svg_start = Instant::now();
        // Render to SVG
        let svg = render_svg(&layout, &theme, &layout_config);
        let svg = retarget_svg_for_png(&svg, target_width, target_height);
        let svg_ms = svg_start.elapsed().as_secs_f32() * 1000.0;

        // Convert SVG to PNG with adaptive dimensions
        let render_config = RenderConfig {
            width: target_width as f32,
            height: target_height as f32,
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
        })
    });

    // Restore the original panic hook
    panic::set_hook(prev_hook);

    // Handle the result
    let render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
    match render_result {
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
            }
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
    }

    // Get actual dimensions from rendered PNG
    let (width, height) =
        get_png_dimensions(&png_path).unwrap_or((target_width_u32, target_height as u32));

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
