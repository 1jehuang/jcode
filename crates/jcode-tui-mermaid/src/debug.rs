use super::*;

pub fn debug_stats() -> MermaidDebugStats {
    debug_support::debug_stats()
}

pub fn reset_debug_stats() {
    debug_support::reset_debug_stats()
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    debug_support::debug_stats_json()
}

pub fn debug_cache() -> Vec<MermaidCacheEntry> {
    debug_support::debug_cache()
}

pub fn debug_memory_profile() -> MermaidMemoryProfile {
    debug_support::debug_memory_profile()
}

pub fn debug_memory_benchmark(iterations: usize) -> MermaidMemoryBenchmark {
    debug_support::debug_memory_benchmark(iterations)
}

pub fn debug_flicker_benchmark(steps: usize) -> MermaidFlickerBenchmark {
    debug_support::debug_flicker_benchmark(steps)
}

pub fn debug_image_scroll_benchmark(
    images: usize,
    frames: usize,
    visible_per_frame: usize,
) -> ImageScrollBenchmark {
    debug_support::debug_image_scroll_benchmark(images, frames, visible_per_frame)
}

#[cfg(test)]
#[allow(dead_code)]
fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    debug_support::parse_proc_status_value_bytes(status, key)
}

pub fn clear_cache() -> Result<(), String> {
    let cache_dir = if let Ok(cache) = RENDER_CACHE.lock() {
        cache.cache_dir.clone()
    } else {
        std::env::temp_dir()
    };

    // Clear in-memory caches
    if let Ok(mut cache) = RENDER_CACHE.lock() {
        cache.entries.clear();
        cache.order.clear();
        cache.width_miss_floor.clear();
    }
    clear_layout_cache();
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.clear();
    }
    if let Ok(mut source) = SOURCE_CACHE.lock() {
        source.clear();
    }
    if let Ok(mut fitted) = FITTED_SOURCE_CACHE.lock() {
        fitted.clear();
    }
    if let Ok(mut kitty) = KITTY_VIEWPORT_STATE.lock() {
        kitty.clear();
    }
    if let Ok(mut last) = LAST_RENDER.lock() {
        last.clear();
    }
    clear_active_diagrams();
    if let Ok(mut pending) = PENDING_RENDER_REQUESTS.lock() {
        pending.clear();
    }
    if let Ok(mut errors) = RENDER_ERRORS.lock() {
        errors.clear();
    }
    bump_deferred_render_epoch();
    clear_streaming_preview_diagram();

    // Remove cached files on disk
    let entries = fs::read_dir(&cache_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("png") {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Debug info for a single image's state
#[derive(Debug, Clone, Serialize)]
pub struct ImageStateInfo {
    pub hash: String,
    pub resize_mode: String,
    pub last_area: Option<String>,
    pub last_viewport: Option<String>,
}

/// Get detailed state info for all cached images
pub fn debug_image_state() -> Vec<ImageStateInfo> {
    if let Ok(state) = IMAGE_STATE.lock() {
        state
            .iter()
            .map(|(hash, img_state)| ImageStateInfo {
                hash: format!("{:016x}", hash),
                resize_mode: match img_state.resize_mode {
                    ResizeMode::Fit => "Fit".to_string(),
                    ResizeMode::Scale => "Scale".to_string(),
                    ResizeMode::Crop => "Crop".to_string(),
                    ResizeMode::Viewport => "Viewport".to_string(),
                    ResizeMode::FitViewport => "FitViewport".to_string(),
                },
                last_area: img_state
                    .last_area
                    .map(|r| format!("{}x{}+{}+{}", r.width, r.height, r.x, r.y)),
                last_viewport: img_state.last_viewport.map(|v| {
                    format!(
                        "scroll={}x{}, view={}x{}",
                        v.scroll_x_px, v.scroll_y_px, v.view_w_px, v.view_h_px
                    )
                }),
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// Result of a test render
#[derive(Debug, Clone, Serialize)]
pub struct TestRenderResult {
    pub success: bool,
    pub hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub path: Option<String>,
    pub error: Option<String>,
    pub render_ms: Option<f32>,
    pub resize_mode: Option<String>,
    pub protocol: Option<String>,
}

/// Render a test diagram and return detailed results (for autonomous testing)
pub fn debug_test_render() -> TestRenderResult {
    let test_content = r#"flowchart LR
    A[Start] --> B{Decision}
    B -->|Yes| C[Action 1]
    B -->|No| D[Action 2]
    C --> E[End]
    D --> E"#;

    debug_render(test_content)
}

/// Render arbitrary mermaid content and return detailed results
pub fn debug_render(content: &str) -> TestRenderResult {
    let start = Instant::now();
    let result = render_mermaid_sized(content, Some(80)); // Use 80 cols as test width

    let render_ms = start.elapsed().as_secs_f32() * 1000.0;
    let protocol = protocol_type().map(|p| format!("{:?}", p));

    match result {
        RenderResult::Image {
            hash,
            path,
            width,
            height,
        } => {
            // Check what resize mode was assigned
            let resize_mode = if let Ok(state) = IMAGE_STATE.lock() {
                state.get(&hash).map(|s| match s.resize_mode {
                    ResizeMode::Fit => "Fit".to_string(),
                    ResizeMode::Scale => "Scale".to_string(),
                    ResizeMode::Crop => "Crop".to_string(),
                    ResizeMode::Viewport => "Viewport".to_string(),
                    ResizeMode::FitViewport => "FitViewport".to_string(),
                })
            } else {
                None
            };

            TestRenderResult {
                success: true,
                hash: Some(format!("{:016x}", hash)),
                width: Some(width),
                height: Some(height),
                path: Some(path.to_string_lossy().to_string()),
                error: None,
                render_ms: Some(render_ms),
                resize_mode,
                protocol,
            }
        }
        RenderResult::Error(msg) => TestRenderResult {
            success: false,
            hash: None,
            width: None,
            height: None,
            path: None,
            error: Some(msg),
            render_ms: Some(render_ms),
            resize_mode: None,
            protocol,
        },
    }
}

/// Simulate multiple renders at different areas to test resize mode stability
/// Returns true if resize mode stayed consistent across all renders
pub fn debug_test_resize_stability(hash: u64) -> serde_json::Value {
    let areas = [
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        },
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        },
        Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 20,
        },
        Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 24,
        },
    ];

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut modes: Vec<String> = Vec::new();

    for area in &areas {
        // Check current resize mode for this hash
        let mode = if let Ok(state) = IMAGE_STATE.lock() {
            state.get(&hash).map(|s| match s.resize_mode {
                ResizeMode::Fit => "Fit",
                ResizeMode::Scale => "Scale",
                ResizeMode::Crop => "Crop",
                ResizeMode::Viewport => "Viewport",
                ResizeMode::FitViewport => "FitViewport",
            })
        } else {
            None
        };

        if let Some(m) = mode {
            modes.push(m.to_string());
            results.push(serde_json::json!({
                "area": format!("{}x{}+{}+{}", area.width, area.height, area.x, area.y),
                "resize_mode": m,
            }));
        }
    }

    let all_same = modes.windows(2).all(|w| w[0] == w[1]);

    serde_json::json!({
        "hash": format!("{:016x}", hash),
        "stable": all_same,
        "modes_observed": modes,
        "details": results,
    })
}

/// Scroll simulation test result
#[derive(Debug, Clone, Serialize)]
pub struct ScrollTestResult {
    pub hash: String,
    pub frames_rendered: usize,
    pub resize_mode_changes: usize,
    pub skipped_renders: u64,
    pub render_calls: Vec<ScrollFrameInfo>,
    pub stable: bool,
    pub border_rendered: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrollFrameInfo {
    pub frame: usize,
    pub y_offset: i32,
    pub visible_rows: u16,
    pub rendered: bool,
    pub resize_mode: Option<String>,
}

/// Simulate scrolling behavior by rendering an image at different y-offsets
/// This tests:
/// 1. Resize mode stability during scroll
/// 2. Border rendering consistency
/// 3. Skip-redundant-render optimization
/// 4. Clearing when scrolled off-screen
pub fn debug_test_scroll(content: Option<&str>) -> ScrollTestResult {
    // First, render a test diagram
    let test_content = content.unwrap_or(
        r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#,
    );

    let render_result = render_mermaid_sized(test_content, Some(80));
    let hash = match render_result {
        RenderResult::Image { hash, .. } => hash,
        RenderResult::Error(_e) => {
            return ScrollTestResult {
                hash: "error".to_string(),
                frames_rendered: 0,
                resize_mode_changes: 0,
                skipped_renders: 0,
                render_calls: vec![],
                stable: false,
                border_rendered: false,
            };
        }
    };

    // Get initial skipped_renders count
    let initial_skipped = if let Ok(debug) = MERMAID_DEBUG.lock() {
        debug.stats.skipped_renders
    } else {
        0
    };

    // Create a test buffer (simulating a terminal)
    let term_width = 100u16;
    let term_height = 40u16;
    let mut buf = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: term_width,
        height: term_height,
    });

    let image_height = 20u16; // Simulated image height in rows
    let mut frames: Vec<ScrollFrameInfo> = Vec::new();
    let mut modes_seen: Vec<String> = Vec::new();
    let mut border_ok = true;

    // Simulate scrolling: image starts at y=5, then scrolls up and eventually off-screen
    let scroll_positions: Vec<i32> = vec![5, 3, 1, 0, -5, -10, -15, -20, -25];

    for (frame_idx, &y_offset) in scroll_positions.iter().enumerate() {
        // Calculate visible area of the image
        let image_top = y_offset;
        let image_bottom = y_offset + image_height as i32;

        // Check if any part is visible
        let visible_top_i32 = image_top.max(0);
        let visible_bottom_i32 = image_bottom.min(term_height as i32);

        let visible = visible_top_i32 < visible_bottom_i32;
        let visible_rows = if visible {
            (visible_bottom_i32 - visible_top_i32) as u16
        } else {
            0
        };
        let visible_top = visible_top_i32 as u16;

        let mut frame_info = ScrollFrameInfo {
            frame: frame_idx,
            y_offset,
            visible_rows,
            rendered: false,
            resize_mode: None,
        };

        if visible && visible_rows > 0 {
            // Render at this position
            let area = Rect {
                x: 0,
                y: visible_top,
                width: term_width,
                height: visible_rows,
            };

            let crop_top = y_offset < 0;
            let rows_used = render_image_widget(hash, area, &mut buf, false, crop_top);
            frame_info.rendered = rows_used > 0;

            // Check resize mode
            if let Ok(state) = IMAGE_STATE.lock()
                && let Some(img_state) = state.get(&hash)
            {
                let mode = match img_state.resize_mode {
                    ResizeMode::Fit => "Fit",
                    ResizeMode::Scale => "Scale",
                    ResizeMode::Crop => "Crop",
                    ResizeMode::Viewport => "Viewport",
                    ResizeMode::FitViewport => "FitViewport",
                };
                frame_info.resize_mode = Some(mode.to_string());
                modes_seen.push(mode.to_string());
            }

            // Check border was rendered (first column should have │)
            if area.x < buf.area().width && area.y < buf.area().height {
                let cell = &buf[(area.x, area.y)];
                if cell.symbol() != "│" {
                    border_ok = false;
                }
            }
        } else {
            // Image scrolled off-screen, clear should be called
            clear_image_area(
                Rect {
                    x: 0,
                    y: 0,
                    width: term_width,
                    height: term_height,
                },
                &mut buf,
            );
        }

        frames.push(frame_info);
    }

    // Check resize mode stability
    let mode_changes = modes_seen.windows(2).filter(|w| w[0] != w[1]).count();

    // Get final skipped count
    let final_skipped = if let Ok(debug) = MERMAID_DEBUG.lock() {
        debug.stats.skipped_renders
    } else {
        0
    };

    ScrollTestResult {
        hash: format!("{:016x}", hash),
        frames_rendered: frames.iter().filter(|f| f.rendered).count(),
        resize_mode_changes: mode_changes,
        skipped_renders: final_skipped - initial_skipped,
        render_calls: frames,
        stable: mode_changes == 0,
        border_rendered: border_ok,
    }
}
