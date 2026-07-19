use super::*;

pub(crate) const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[allow(clippy::too_many_arguments)]
pub(crate) fn single_session_streaming_primitive_geometry_cache_key(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
    tool_motion_cache_key: u64,
    inline_widget_list_reflow_cache_key: u64,
    inline_widget_preview_pane_cache_key: u64,
    composer_motion_cache_key: u64,
    attachment_chip_motion_cache_key: u64,
    stdin_overlay_motion_cache_key: u64,
    transcript_message_motion_cache_key: u64,
    transcript_motion_cache_key: u64,
    inline_markdown_motion_cache_key: u64,
    activity_cue_motion_cache_key: u64,
    scrollbar_motion_cache_key: u64,
    body_key: Option<u64>,
    body_line_count: usize,
) -> Option<u64> {
    let body_key = body_key?;
    if app.streaming_response.is_empty()
        || app.show_help
        || app.model_picker.open
        || app.model_picker.loading
        || app.session_switcher.open
        || app.session_switcher.loading
        || app.render_inline_widget_line_count() > 0
        || app.stdin_response.is_some()
        || app.has_active_selection()
        || app.is_welcome_timeline_visible()
    {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    (size.width, size.height).hash(&mut hasher);
    app.text_scale().to_bits().hash(&mut hasher);
    app.body_scroll_lines.to_bits().hash(&mut hasher);
    smooth_scroll_lines.to_bits().hash(&mut hasher);
    focus_pulse.to_bits().hash(&mut hasher);
    welcome_hero_reveal_progress.to_bits().hash(&mut hasher);
    tool_motion_cache_key.hash(&mut hasher);
    inline_widget_list_reflow_cache_key.hash(&mut hasher);
    inline_widget_preview_pane_cache_key.hash(&mut hasher);
    composer_motion_cache_key.hash(&mut hasher);
    attachment_chip_motion_cache_key.hash(&mut hasher);
    stdin_overlay_motion_cache_key.hash(&mut hasher);
    transcript_message_motion_cache_key.hash(&mut hasher);
    transcript_motion_cache_key.hash(&mut hasher);
    inline_markdown_motion_cache_key.hash(&mut hasher);
    activity_cue_motion_cache_key.hash(&mut hasher);
    scrollbar_motion_cache_key.hash(&mut hasher);
    spinner_tick.hash(&mut hasher);
    app.is_processing.hash(&mut hasher);
    app.status.hash(&mut hasher);
    app.error.hash(&mut hasher);
    app.pending_images.len().hash(&mut hasher);
    app.messages.len().hash(&mut hasher);
    app.draft.len().hash(&mut hasher);
    app.draft_cursor.hash(&mut hasher);
    app.draft
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        .hash(&mut hasher);
    body_key.hash(&mut hasher);
    body_line_count.hash(&mut hasher);
    Some(hasher.finish())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AppModeTransitionFrame {
    pub(crate) previous_opacity: f32,
    pub(crate) previous_scale: f32,
    pub(crate) current_opacity: f32,
    pub(crate) current_scale: f32,
}

#[derive(Default)]
pub(crate) struct AppModeTransitionState {
    pub(crate) last_mode: Option<&'static str>,
    pub(crate) started_at: Option<Instant>,
    pub(crate) previous_vertices: Vec<Vertex>,
    pub(crate) last_vertices: Vec<Vertex>,
}

impl AppModeTransitionState {
    pub(crate) fn frame(
        &mut self,
        mode: &'static str,
        now: Instant,
    ) -> Option<AppModeTransitionFrame> {
        if animation::desktop_reduced_motion_enabled() {
            self.last_mode = Some(mode);
            self.started_at = None;
            self.previous_vertices.clear();
            return None;
        }

        match self.last_mode {
            None => {
                self.last_mode = Some(mode);
                return None;
            }
            Some(previous_mode) if previous_mode != mode => {
                self.last_mode = Some(mode);
                self.previous_vertices.clear();
                self.previous_vertices
                    .extend_from_slice(&self.last_vertices);
                self.started_at = (!self.previous_vertices.is_empty()).then_some(now);
            }
            Some(_) => {}
        }

        let started_at = self.started_at?;
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / APP_MODE_TRANSITION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.started_at = None;
            self.previous_vertices.clear();
            return None;
        }

        let eased = animation::ease_out_cubic(progress);
        Some(AppModeTransitionFrame {
            previous_opacity: 1.0 - eased,
            previous_scale: animation::lerp(1.0, 0.985, eased),
            current_opacity: eased,
            current_scale: animation::lerp(0.985, 1.0, eased),
        })
    }

    pub(crate) fn previous_vertices(&self) -> &[Vertex] {
        &self.previous_vertices
    }

    pub(crate) fn remember_uploaded_vertices(&mut self, vertices: &[Vertex]) {
        self.last_vertices.clear();
        self.last_vertices.extend_from_slice(vertices);
    }

    pub(crate) fn clear(&mut self) {
        self.last_mode = None;
        self.started_at = None;
        self.previous_vertices.clear();
        self.last_vertices.clear();
    }
}

pub(crate) fn compose_app_mode_transition_vertices(
    output: &mut Vec<Vertex>,
    previous_vertices: &[Vertex],
    current_vertices: &[Vertex],
    frame: AppModeTransitionFrame,
) {
    output.clear();
    append_app_mode_transition_vertices(
        output,
        previous_vertices,
        frame.previous_opacity,
        frame.previous_scale,
    );
    append_app_mode_transition_vertices(
        output,
        current_vertices,
        frame.current_opacity,
        frame.current_scale,
    );
}

pub(crate) fn append_app_mode_transition_vertices(
    output: &mut Vec<Vertex>,
    vertices: &[Vertex],
    opacity: f32,
    scale: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    output.extend(vertices.iter().map(|vertex| {
        let mut color = vertex.color;
        color[3] *= opacity;
        Vertex {
            position: [vertex.position[0] * scale, vertex.position[1] * scale],
            color,
        }
    }));
}

pub(crate) struct Canvas {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) render_pipeline: Option<wgpu::RenderPipeline>,
    pub(crate) hero_mask_renderer: Option<HeroMaskRenderer>,
    pub(crate) font_system_loader: Option<JoinHandle<FontSystem>>,
    pub(crate) font_system: Option<FontSystem>,
    pub(crate) swash_cache: SwashCache,
    pub(crate) text_atlas: Option<TextAtlas>,
    pub(crate) text_renderer: Option<TextRenderer>,
    pub(crate) text_needs_prepare: bool,
    pub(crate) streaming_text_atlas: Option<TextAtlas>,
    pub(crate) streaming_text_renderer: Option<TextRenderer>,
    pub(crate) streaming_text_needs_prepare: bool,
    pub(crate) size: PhysicalSize<u32>,
    pub(crate) surface_zero_sized: bool,
    pub(crate) viewport_animation: AnimatedViewport,
    pub(crate) surface_transitions: SurfaceTransitionAnimator,
    pub(crate) workspace_surface_exit_cache: HashMap<u64, workspace::Surface>,
    pub(crate) workspace_text_pane_cache: HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    pub(crate) focus_pulse: FocusPulse,
    pub(crate) status_color_transition: ColorTransition,
    pub(crate) status_text_transition: StatusTextTransition,
    pub(crate) inline_widget_selection_motion: InlineWidgetSelectionMotionRegistry,
    pub(crate) inline_widget_list_reflow_motion: InlineWidgetListReflowMotionRegistry,
    pub(crate) inline_widget_preview_pane_motion: InlineWidgetPreviewPaneMotionRegistry,
    pub(crate) composer_motion: ComposerMotionRegistry,
    pub(crate) attachment_chip_motion: AttachmentChipMotionRegistry,
    pub(crate) stdin_overlay_motion: StdinOverlayMotionRegistry,
    pub(crate) transcript_card_motion: TranscriptCardMotionRegistry,
    pub(crate) inline_markdown_pill_motion: InlineMarkdownPillMotionRegistry,
    pub(crate) streaming_activity_cue_motion: StreamingActivityCueMotionRegistry,
    pub(crate) tool_card_motion: ToolCardMotionRegistry,
    pub(crate) single_session_scrollbar_motion: SingleSessionScrollbarMotionRegistry,
    pub(crate) primitive_vertex_buffer: Option<wgpu::Buffer>,
    pub(crate) primitive_vertex_capacity: usize,
    pub(crate) primitive_vertices_cache_key: Option<u64>,
    pub(crate) primitive_vertices_cache: Vec<Vertex>,
    pub(crate) primitive_frame_vertices: Vec<Vertex>,
    pub(crate) primitive_caret_vertices: Vec<Vertex>,
    pub(crate) primitive_workspace_vertices: Vec<Vertex>,
    pub(crate) primitive_workspace_vertices_cache_key: Option<u64>,
    pub(crate) app_mode_transition: AppModeTransitionState,
    pub(crate) app_mode_transition_vertices: Vec<Vertex>,
    pub(crate) single_session_scroll_motion: SingleSessionScrollMotion,
    pub(crate) streaming_follow_motion: StreamingFollowMotion,
    pub(crate) transcript_message_motion: TranscriptMessageMotionRegistry,
    pub(crate) needs_initial_frame: bool,
    pub(crate) boot_frame_presented: bool,
    pub(crate) first_render_completed: bool,
    pub(crate) defer_initial_text_frame: bool,
    pub(crate) single_session_text_cache_key: Option<u64>,
    pub(crate) single_session_text_key: Option<SingleSessionTextKey>,
    pub(crate) single_session_text_buffers: Vec<Buffer>,
    pub(crate) single_session_raw_body_key: Option<u64>,
    pub(crate) single_session_raw_body_lines: Vec<SingleSessionStyledLine>,
    pub(crate) single_session_body_key: Option<u64>,
    pub(crate) single_session_body_lines: Vec<SingleSessionStyledLine>,
    pub(crate) single_session_streaming_base_key: Option<u64>,
    pub(crate) single_session_streaming_base_len: usize,
    pub(crate) single_session_streaming_response_len: usize,
    pub(crate) single_session_streaming_fade_started_at: Option<Instant>,
    pub(crate) single_session_streaming_handoff_started_at: Option<Instant>,
    pub(crate) streaming_text_reveal: StreamingTextRevealMotion,
    pub(crate) single_session_streaming_reveal_frame: StreamingTextRevealFrame,
    pub(crate) single_session_streaming_revealed_bytes: usize,
    pub(crate) single_session_streaming_text_key: Option<u64>,
    pub(crate) single_session_streaming_text_start_line: Option<usize>,
    pub(crate) single_session_streaming_text_end_line: Option<usize>,
    pub(crate) single_session_streaming_text_opacity_bits: Option<u32>,
    pub(crate) single_session_streaming_text_buffer: Option<Buffer>,
    pub(crate) single_session_body_text_scroll_start: Option<usize>,
    pub(crate) single_session_body_text_top_offset_bits: Option<u32>,
    pub(crate) single_session_body_text_window_start: Option<usize>,
    pub(crate) single_session_body_text_window_end: Option<usize>,
    pub(crate) welcome_hero_reveal_key: Option<String>,
    pub(crate) welcome_hero_reveal_started_at: Option<Instant>,
    pub(crate) frame_profiler: DesktopFrameProfiler,
}

impl Canvas {
    pub(crate) async fn new(
        window: Arc<Window>,
        startup_trace: DesktopStartupTrace,
    ) -> Result<Self> {
        let initial_window_size = window.inner_size();
        let size = non_zero_size(initial_window_size);
        let font_system_loader = Some(spawn_desktop_font_system_loader());
        startup_trace.mark("font loader spawned");
        let (surface, adapter) = request_startup_adapter(
            window.clone(),
            desktop_wgpu_startup_backends(),
            startup_trace,
        )
        .await?;
        startup_trace.mark("wgpu adapter ready");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("jcode-desktop-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("failed to create wgpu device")?;
        startup_trace.mark("wgpu device ready");
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        // Prefer Mailbox for low-latency presentation (latest frame replaces any
        // queued frame, so scroll/redraw updates show up on the very next vblank
        // without tearing). Fall back to Fifo (hard vsync) and finally to whatever
        // the surface advertises first.
        let present_mode = if capabilities.present_modes.contains(&PresentMode::Mailbox) {
            PresentMode::Mailbox
        } else if capabilities.present_modes.contains(&PresentMode::Fifo) {
            PresentMode::Fifo
        } else {
            capabilities.present_modes[0]
        };
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&CompositeAlphaMode::Opaque)
        {
            CompositeAlphaMode::Opaque
        } else {
            capabilities.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            // One in-flight frame keeps input-to-photon latency low. With 2+ the
            // GPU/compositor can queue an extra frame, adding ~16ms of perceived
            // scroll lag on a 60Hz display.
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);
        startup_trace.mark("surface configured");
        startup_trace.mark("first-frame GPU core ready");
        let swash_cache = SwashCache::new();
        Ok(Self {
            surface,
            device,
            queue,
            config,
            render_pipeline: None,
            hero_mask_renderer: None,
            font_system_loader,
            font_system: None,
            swash_cache,
            text_atlas: None,
            text_renderer: None,
            text_needs_prepare: true,
            streaming_text_atlas: None,
            streaming_text_renderer: None,
            streaming_text_needs_prepare: false,
            size,
            surface_zero_sized: !desktop_surface_size_is_renderable(initial_window_size),
            viewport_animation: AnimatedViewport::default(),
            surface_transitions: SurfaceTransitionAnimator::default(),
            workspace_surface_exit_cache: HashMap::new(),
            workspace_text_pane_cache: HashMap::new(),
            focus_pulse: FocusPulse::default(),
            status_color_transition: ColorTransition::default(),
            status_text_transition: StatusTextTransition::default(),
            inline_widget_selection_motion: InlineWidgetSelectionMotionRegistry::default(),
            inline_widget_list_reflow_motion: InlineWidgetListReflowMotionRegistry::default(),
            inline_widget_preview_pane_motion: InlineWidgetPreviewPaneMotionRegistry::default(),
            composer_motion: ComposerMotionRegistry::default(),
            attachment_chip_motion: AttachmentChipMotionRegistry::default(),
            stdin_overlay_motion: StdinOverlayMotionRegistry::default(),
            transcript_card_motion: TranscriptCardMotionRegistry::default(),
            inline_markdown_pill_motion: InlineMarkdownPillMotionRegistry::default(),
            streaming_activity_cue_motion: StreamingActivityCueMotionRegistry::default(),
            tool_card_motion: ToolCardMotionRegistry::default(),
            single_session_scrollbar_motion: SingleSessionScrollbarMotionRegistry::default(),
            primitive_vertex_buffer: None,
            primitive_vertex_capacity: 0,
            primitive_vertices_cache_key: None,
            primitive_vertices_cache: Vec::new(),
            primitive_frame_vertices: Vec::new(),
            primitive_caret_vertices: Vec::new(),
            primitive_workspace_vertices: Vec::new(),
            primitive_workspace_vertices_cache_key: None,
            app_mode_transition: AppModeTransitionState::default(),
            app_mode_transition_vertices: Vec::new(),
            single_session_scroll_motion: SingleSessionScrollMotion::default(),
            streaming_follow_motion: StreamingFollowMotion::default(),
            transcript_message_motion: TranscriptMessageMotionRegistry::default(),
            needs_initial_frame: true,
            boot_frame_presented: false,
            first_render_completed: false,
            defer_initial_text_frame: false,
            single_session_text_cache_key: None,
            single_session_text_key: None,
            single_session_text_buffers: Vec::new(),
            single_session_raw_body_key: None,
            single_session_raw_body_lines: Vec::new(),
            single_session_body_key: None,
            single_session_body_lines: Vec::new(),
            single_session_streaming_base_key: None,
            single_session_streaming_base_len: 0,
            single_session_streaming_response_len: 0,
            single_session_streaming_fade_started_at: None,
            single_session_streaming_handoff_started_at: None,
            streaming_text_reveal: StreamingTextRevealMotion::default(),
            single_session_streaming_reveal_frame: StreamingTextRevealFrame::default(),
            single_session_streaming_revealed_bytes: 0,
            single_session_streaming_text_key: None,
            single_session_streaming_text_start_line: None,
            single_session_streaming_text_end_line: None,
            single_session_streaming_text_opacity_bits: None,
            single_session_streaming_text_buffer: None,
            single_session_body_text_scroll_start: None,
            single_session_body_text_top_offset_bits: None,
            single_session_body_text_window_start: None,
            single_session_body_text_window_end: None,
            welcome_hero_reveal_key: None,
            welcome_hero_reveal_started_at: None,
            frame_profiler: DesktopFrameProfiler::new(),
        })
    }

    pub(crate) fn suspend_for_zero_size(&mut self, size: PhysicalSize<u32>) {
        if !self.surface_zero_sized {
            desktop_log::info(format_args!(
                "jcode-desktop: suspending surface rendering while window is zero-sized ({}x{})",
                size.width, size.height
            ));
        }
        self.surface_zero_sized = true;
    }

    pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
        if !desktop_surface_size_is_renderable(size) {
            self.suspend_for_zero_size(size);
            return;
        }

        let was_zero_sized = self.surface_zero_sized;
        self.surface_zero_sized = false;
        if was_zero_sized {
            desktop_log::info(format_args!(
                "jcode-desktop: resuming surface rendering at {}x{}",
                size.width, size.height
            ));
        }

        if self.size == size && !was_zero_sized {
            return;
        }

        self.size = size;
        self.primitive_vertices_cache_key = None;
        self.primitive_vertices_cache.clear();
        self.primitive_frame_vertices.clear();
        self.primitive_caret_vertices.clear();
        // Keep workspace pane entries across resizes. Their identity keys include
        // the panel dimensions, so the next render refreshes any size-sensitive
        // text and geometry while retaining the previous buffers for compatible
        // layout buckets. Clearing here made every intermediate resize frame a
        // cold render and dropped all glyph/vertex allocations.
        self.app_mode_transition.clear();
        self.app_mode_transition_vertices.clear();
        self.single_session_scroll_motion.clear();
        self.transcript_message_motion.clear();
        self.inline_widget_preview_pane_motion.clear();
        self.streaming_activity_cue_motion.clear();
        self.single_session_streaming_handoff_started_at = None;
        self.first_render_completed = false;
        self.text_needs_prepare = true;
        if self.single_session_streaming_text_buffer.is_some() {
            self.streaming_text_needs_prepare = true;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub(crate) fn refresh_cached_single_session_text_buffers(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        now: Instant,
        smooth_scroll_lines: f32,
        rendered_body_key: u64,
        rendered_body_changed: bool,
    ) {
        let tick = desktop_spinner_tick(now);
        let viewport = single_session_body_viewport_from_lines(
            app,
            render_size,
            smooth_scroll_lines,
            &self.single_session_body_lines,
        );
        let text_cache_key =
            single_session_text_buffer_cache_key(app, render_size, tick, rendered_body_key);
        let key = single_session_text_key_for_tick_with_rendered_body(
            app,
            render_size,
            tick,
            smooth_scroll_lines,
            &self.single_session_body_lines,
        );
        let text_key_changed = self.single_session_text_key.as_ref() != Some(&key);
        if self.single_session_text_cache_key != Some(text_cache_key) || text_key_changed {
            let desired_body_window = self.single_session_body_buffer_window_bounds(app, &viewport);
            let body_window_contains = if let (Some(window_start), Some(window_end)) = (
                self.single_session_body_text_window_start,
                self.single_session_body_text_window_end,
            ) {
                self.single_session_body_buffer_window_contains(
                    app,
                    window_start,
                    window_end,
                    &viewport,
                )
            } else {
                false
            };
            let Some(font_system) = self.font_system.as_mut() else {
                self.single_session_text_cache_key = None;
                self.single_session_text_key = None;
                self.single_session_text_buffers.clear();
                self.single_session_body_text_scroll_start = None;
                self.single_session_body_text_top_offset_bits = None;
                self.single_session_body_text_window_start = None;
                self.single_session_body_text_window_end = None;
                return;
            };
            let previous_key = self.single_session_text_key.take();
            let mut old_buffers = std::mem::take(&mut self.single_session_text_buffers);
            let body_content_changed_in_buffer =
                rendered_body_changed && app.streaming_response.is_empty();
            let body_layout_compatible = previous_key.as_ref().is_some_and(|previous| {
                single_session_body_text_buffer_layout_compatible(
                    previous.size,
                    render_size,
                    app.text_scale(),
                )
            });
            let mut can_reuse_body_buffer = old_buffers.len() > 1
                && body_window_contains
                && !body_content_changed_in_buffer
                && body_layout_compatible;
            if old_buffers.len() > 1
                && (!body_window_contains
                    || body_content_changed_in_buffer
                    || !body_layout_compatible)
            {
                let (window_start, window_end) = desired_body_window;
                old_buffers[1] = single_session_body_text_buffer_from_lines(
                    font_system,
                    &self.single_session_body_lines[window_start..window_end],
                    render_size,
                    app.text_scale(),
                );
                self.single_session_body_text_window_start = Some(window_start);
                self.single_session_body_text_window_end = Some(window_end);
                self.single_session_body_text_scroll_start = None;
                can_reuse_body_buffer = true;
            }
            self.single_session_text_buffers =
                single_session_text_buffers_from_key_reusing_unchanged(
                    &key,
                    previous_key.as_ref(),
                    old_buffers,
                    can_reuse_body_buffer,
                    render_size,
                    font_system,
                );
            self.single_session_text_key = Some(key);
            self.single_session_text_cache_key = Some(text_cache_key);
            if !can_reuse_body_buffer {
                self.single_session_body_text_scroll_start = None;
                self.single_session_body_text_top_offset_bits = None;
                self.single_session_body_text_window_start = None;
                self.single_session_body_text_window_end = None;
            }
            self.text_needs_prepare = true;
        }
        self.sync_single_session_body_text_window(app, render_size, &viewport);
        self.sync_single_session_body_text_top_offset(viewport.top_offset_pixels);
    }

    pub(crate) fn sync_single_session_body_text_top_offset(&mut self, top_offset_pixels: f32) {
        let bits = top_offset_pixels.to_bits();
        if self.single_session_body_text_top_offset_bits == Some(bits) {
            return;
        }
        self.single_session_body_text_top_offset_bits = Some(bits);
        self.text_needs_prepare = true;
    }

    pub(crate) fn sync_single_session_body_text_window(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        viewport: &SingleSessionBodyViewport,
    ) {
        let desired_body_window = self.single_session_body_buffer_window_bounds(app, viewport);
        if let (Some(window_start), Some(window_end)) = (
            self.single_session_body_text_window_start,
            self.single_session_body_text_window_end,
        ) && self.single_session_body_buffer_window_contains(
            app,
            window_start,
            window_end,
            viewport,
        ) {
            self.sync_single_session_body_text_scroll(viewport.start_line, window_start);
            self.sync_single_session_streaming_text_buffer(app, render_size, viewport);
            return;
        }

        let (window_start, window_end) = desired_body_window;
        let window_lines = self.single_session_body_lines[window_start..window_end].to_vec();
        if let Some(font_system) = self.font_system.as_mut()
            && let Some(body_buffer) = self.single_session_text_buffers.get_mut(1)
        {
            *body_buffer = single_session_body_text_buffer_from_lines(
                font_system,
                &window_lines,
                render_size,
                app.text_scale(),
            );
            self.single_session_body_text_window_start = Some(window_start);
            self.single_session_body_text_window_end = Some(window_end);
            self.single_session_body_text_scroll_start = None;
            self.single_session_body_text_top_offset_bits = None;
            self.sync_single_session_body_text_scroll(viewport.start_line, window_start);
        }
        self.sync_single_session_streaming_text_buffer(app, render_size, viewport);
    }

    pub(crate) fn single_session_body_buffer_window_bounds(
        &self,
        app: &SingleSessionApp,
        viewport: &SingleSessionBodyViewport,
    ) -> (usize, usize) {
        let (window_start, window_end) = single_session_body_text_window_bounds(viewport);
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return (window_start, window_end);
        }
        let visible_static_start = viewport
            .start_line
            .min(self.single_session_streaming_base_len);
        let visible_static_end = viewport
            .start_line
            .saturating_add(viewport.lines.len())
            .min(self.single_session_streaming_base_len);
        let start = visible_static_start
            .saturating_sub(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_BEFORE_LINES);
        let end = visible_static_end
            .saturating_add(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_AFTER_LINES)
            .min(self.single_session_streaming_base_len)
            .max(start);
        (start, end)
    }

    pub(crate) fn single_session_body_buffer_window_contains(
        &self,
        app: &SingleSessionApp,
        window_start: usize,
        window_end: usize,
        viewport: &SingleSessionBodyViewport,
    ) -> bool {
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return single_session_body_text_window_contains(window_start, window_end, viewport);
        }
        let (desired_start, desired_end) =
            self.single_session_body_buffer_window_bounds(app, viewport);
        window_start == desired_start && window_end == desired_end
    }

    pub(crate) fn sync_single_session_streaming_text_buffer(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        viewport: &SingleSessionBodyViewport,
    ) {
        self.update_single_session_streaming_fade(app);
        let Some((start_line, end_line)) =
            self.single_session_streaming_visible_range(app, viewport)
        else {
            if app.streaming_response.is_empty()
                && self.single_session_streaming_handoff_started_at.is_some()
                && self.single_session_streaming_text_buffer.is_some()
            {
                self.streaming_text_needs_prepare = true;
                return;
            }
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_needs_prepare = false;
            return;
        };

        let tail_fade_chars = self.single_session_streaming_reveal_frame.tail_fade_chars;
        // Quantize so the cache key only changes when the fade visibly moves.
        let tail_fade_quantized = (tail_fade_chars * 4.0).round() as u32;
        let mut hasher = DefaultHasher::new();
        (render_size.width, render_size.height).hash(&mut hasher);
        app.text_scale().to_bits().hash(&mut hasher);
        start_line.hash(&mut hasher);
        end_line.hash(&mut hasher);
        tail_fade_quantized.hash(&mut hasher);
        self.single_session_body_lines[start_line..end_line].hash(&mut hasher);
        let key = hasher.finish();
        if self.single_session_streaming_text_key == Some(key) {
            return;
        }

        if let Some(font_system) = self.font_system.as_mut() {
            let lines = self.single_session_body_lines[start_line..end_line].to_vec();
            self.single_session_streaming_text_buffer = Some(
                single_session_body_text_buffer_from_lines_with_opacity_and_tail_fade(
                    font_system,
                    &lines,
                    render_size,
                    app.text_scale(),
                    1.0,
                    tail_fade_quantized as f32 / 4.0,
                ),
            );
            self.single_session_streaming_text_key = Some(key);
            self.single_session_streaming_text_start_line = Some(start_line);
            self.single_session_streaming_text_end_line = Some(end_line);
            self.single_session_streaming_text_opacity_bits = Some(1.0f32.to_bits());
            self.streaming_text_needs_prepare = true;
        }
    }

    pub(crate) fn update_single_session_streaming_fade(&mut self, app: &SingleSessionApp) {
        let now = Instant::now();
        let previous_len = self.single_session_streaming_response_len;
        let response_len = app.streaming_response.len();
        let has_visible_streaming_buffer = self.single_session_streaming_text_buffer.is_some()
            && self.single_session_streaming_text_start_line.is_some()
            && self.single_session_streaming_text_end_line.is_some();
        self.single_session_streaming_handoff_started_at =
            streaming_text_handoff_start_after_len_change(
                previous_len,
                response_len,
                has_visible_streaming_buffer,
                self.single_session_streaming_handoff_started_at,
                now,
            );
        self.single_session_streaming_fade_started_at = streaming_text_fade_start_after_len_change(
            previous_len,
            response_len,
            self.single_session_streaming_fade_started_at,
            now,
        );
        self.single_session_streaming_response_len = response_len;
    }

    pub(crate) fn single_session_streaming_arrival_style(
        &mut self,
        now: Instant,
    ) -> StreamingTextArrivalStyle {
        if let Some(started_at) = self.single_session_streaming_handoff_started_at {
            let style =
                streaming_text_handoff_style_for_elapsed(now.saturating_duration_since(started_at));
            if style.active {
                return style;
            }
            self.single_session_streaming_handoff_started_at = None;
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.streaming_text_needs_prepare = false;
            return style;
        }

        let Some(started_at) = self.single_session_streaming_fade_started_at else {
            return StreamingTextArrivalStyle {
                opacity: 1.0,
                y_offset_pixels: 0.0,
                active: false,
            };
        };
        let style =
            streaming_text_arrival_style_for_elapsed(now.saturating_duration_since(started_at));
        if !style.active {
            self.single_session_streaming_fade_started_at = None;
            return StreamingTextArrivalStyle {
                opacity: 1.0,
                y_offset_pixels: 0.0,
                active: false,
            };
        }
        style
    }

    pub(crate) fn update_single_session_streaming_text_buffer_opacity(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        opacity: f32,
    ) {
        let opacity = opacity.clamp(0.0, 1.0);
        let quantized_opacity = (opacity * 255.0).round() / 255.0;
        let opacity_bits = quantized_opacity.to_bits();
        if self.single_session_streaming_text_opacity_bits == Some(opacity_bits) {
            return;
        }
        let (Some(start_line), Some(end_line), Some(font_system)) = (
            self.single_session_streaming_text_start_line,
            self.single_session_streaming_text_end_line,
            self.font_system.as_mut(),
        ) else {
            return;
        };
        if start_line >= end_line || end_line > self.single_session_body_lines.len() {
            return;
        }

        let lines = self.single_session_body_lines[start_line..end_line].to_vec();
        self.single_session_streaming_text_buffer =
            Some(single_session_body_text_buffer_from_lines_with_opacity(
                font_system,
                &lines,
                render_size,
                app.text_scale(),
                quantized_opacity,
            ));
        self.single_session_streaming_text_opacity_bits = Some(opacity_bits);
        self.streaming_text_needs_prepare = true;
    }

    pub(crate) fn single_session_streaming_visible_range(
        &self,
        app: &SingleSessionApp,
        viewport: &SingleSessionBodyViewport,
    ) -> Option<(usize, usize)> {
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return None;
        }
        let streaming_start_line = self
            .single_session_streaming_base_len
            .saturating_add(usize::from(!app.messages.is_empty()));
        let visible_start = viewport.start_line;
        let visible_end = viewport.start_line.saturating_add(viewport.lines.len());
        let start = streaming_start_line.max(visible_start);
        let end = self.single_session_body_lines.len().min(visible_end);
        (start < end).then_some((start, end))
    }

    pub(crate) fn sync_single_session_body_text_scroll(
        &mut self,
        start_line: usize,
        window_start: usize,
    ) {
        if self.single_session_body_text_scroll_start == Some(start_line) {
            return;
        }
        if let Some(body_buffer) = self.single_session_text_buffers.get_mut(1) {
            body_buffer.set_scroll(
                start_line
                    .saturating_sub(window_start)
                    .min(i32::MAX as usize) as i32,
            );
            self.single_session_body_text_scroll_start = Some(start_line);
            self.text_needs_prepare = true;
        }
    }

    pub(crate) fn ensure_font_system(&mut self) {
        if self.font_system.is_some() {
            return;
        }
        self.font_system = Some(
            self.font_system_loader
                .take()
                .and_then(|loader| match loader.join() {
                    Ok(font_system) => Some(font_system),
                    Err(_) => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: font system loader thread panicked"
                        ));
                        None
                    }
                })
                .unwrap_or_else(create_desktop_font_system),
        );
    }

    pub(crate) fn ensure_render_pipeline(&mut self) {
        if self.render_pipeline.is_some() {
            return;
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("jcode-desktop-primitive-shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("jcode-desktop-primitive-pipeline-layout"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        self.render_pipeline = Some(self.device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("jcode-desktop-primitive-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[Vertex::layout()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            },
        ));
    }

    pub(crate) fn ensure_hero_mask_renderer(&mut self) {
        if self.hero_mask_renderer.is_some() {
            return;
        }
        self.hero_mask_renderer = Some(HeroMaskRenderer::new(&self.device, self.config.format));
    }

    pub(crate) fn ensure_text_renderer(&mut self) {
        if self.text_renderer.is_some() {
            return;
        }
        let mut text_atlas = TextAtlas::new(&self.device, &self.queue, self.config.format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &self.device,
            wgpu::MultisampleState::default(),
            None,
        );
        self.text_atlas = Some(text_atlas);
        self.text_renderer = Some(text_renderer);
        self.text_needs_prepare = true;
    }

    pub(crate) fn ensure_streaming_text_renderer(&mut self) {
        if self.streaming_text_renderer.is_some() {
            return;
        }
        let mut text_atlas = TextAtlas::new(&self.device, &self.queue, self.config.format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &self.device,
            wgpu::MultisampleState::default(),
            None,
        );
        self.streaming_text_atlas = Some(text_atlas);
        self.streaming_text_renderer = Some(text_renderer);
        self.streaming_text_needs_prepare = true;
    }

    pub(crate) fn release_streaming_text_renderer_if_idle(
        &mut self,
        has_streaming_text_buffer: bool,
    ) {
        if !streaming_text_renderer_should_release(
            has_streaming_text_buffer,
            self.streaming_text_renderer.is_some(),
            self.streaming_text_atlas.is_some(),
        ) {
            return;
        }

        self.streaming_text_renderer = None;
        self.streaming_text_atlas = None;
        self.streaming_text_needs_prepare = false;
    }

    pub(crate) fn cached_single_session_body_lines(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        tick: u64,
    ) -> (u64, bool) {
        let body_layout_size = single_session_body_layout_cache_size(app, render_size);
        let key = streaming_reveal_body_cache_key(
            app.rendered_body_cache_key(body_layout_size),
            app.streaming_response.is_empty(),
            self.single_session_streaming_revealed_bytes,
        );
        if self.single_session_body_key == Some(key) {
            return (key, false);
        }

        if !app.streaming_response.is_empty() {
            self.single_session_raw_body_key = None;
            self.single_session_raw_body_lines.clear();
            let base_key = app.rendered_body_static_cache_key(body_layout_size);
            if self.single_session_streaming_base_key != Some(base_key) {
                if let Some(base_lines) =
                    single_session_rendered_static_body_lines_for_streaming(app, render_size, tick)
                {
                    self.single_session_body_lines = base_lines;
                    self.single_session_streaming_base_len = self.single_session_body_lines.len();
                    self.single_session_streaming_base_key = Some(base_key);
                    self.single_session_body_text_scroll_start = None;
                    self.single_session_body_text_top_offset_bits = None;
                    self.single_session_body_text_window_start = None;
                    self.single_session_body_text_window_end = None;
                } else {
                    self.single_session_body_lines =
                        single_session_rendered_body_lines_for_tick(app, render_size, tick);
                    self.single_session_streaming_base_key = None;
                    self.single_session_streaming_base_len = 0;
                    self.single_session_body_key = Some(key);
                    self.single_session_body_text_scroll_start = None;
                    self.single_session_body_text_top_offset_bits = None;
                    self.single_session_body_text_window_start = None;
                    self.single_session_body_text_window_end = None;
                    return (key, true);
                }
            } else {
                self.single_session_body_lines
                    .truncate(self.single_session_streaming_base_len);
            }
            append_single_session_streaming_response_rendered_body_lines_with_reveal(
                app,
                render_size,
                &mut self.single_session_body_lines,
                self.single_session_streaming_revealed_bytes,
            );
        } else {
            let raw_key = app.rendered_body_cache_key((0, 0));
            if self.single_session_raw_body_key != Some(raw_key) {
                self.single_session_raw_body_lines = app.body_styled_lines_for_tick(tick);
                self.single_session_raw_body_key = Some(raw_key);
            }
            self.single_session_body_lines = single_session_rendered_body_lines_from_raw_ref(
                app,
                render_size,
                &self.single_session_raw_body_lines,
            );
            self.single_session_streaming_base_key = None;
            self.single_session_streaming_base_len = 0;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
        }
        self.single_session_body_key = Some(key);
        self.single_session_body_text_scroll_start = None;
        (key, true)
    }

    pub(crate) fn welcome_hero_reveal_progress(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> (f32, bool) {
        if !app.is_welcome_timeline_visible() {
            self.welcome_hero_reveal_key = None;
            self.welcome_hero_reveal_started_at = None;
            return (1.0, false);
        }

        let key = app.welcome_hero_text();
        if self.welcome_hero_reveal_key.as_deref() != Some(key.as_str()) {
            self.welcome_hero_reveal_key = Some(key);
            self.welcome_hero_reveal_started_at = None;
        }

        let elapsed = self
            .welcome_hero_reveal_started_at
            .map(|started_at| now.saturating_duration_since(started_at))
            .unwrap_or_default();
        let progress = welcome_hero_reveal_progress_for_elapsed(elapsed);
        (progress, welcome_hero_reveal_is_active(progress))
    }

    pub(crate) fn render_boot_frame(
        &mut self,
    ) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-boot-frame"),
            });
        frame_profile.checkpoint("frame_setup");
        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-boot-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        frame_profile.checkpoint("render_pass");
        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        self.boot_frame_presented = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: "boot",
            smooth_scroll_lines: 0.0,
            text_buffer_count: 0,
            text_area_count: 0,
            primitive_vertices: 0,
            body_line_count: 0,
            viewport_line_count: 0,
            body_text_window_line_count: 0,
            streaming_text_line_count: 0,
            inline_widget_line_count: 0,
            text_prepared: false,
            primitive_geometry_cache_hit: false,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: true,
            content_ready: false,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn render_scene(
        &mut self,
        scene: &DesktopScene,
    ) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        if !self.boot_frame_presented {
            return self.render_boot_frame();
        }

        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-scene-render"),
            });
        frame_profile.checkpoint("frame_setup");

        self.ensure_render_pipeline();
        frame_profile.checkpoint("primitive_pipeline");
        self.primitive_frame_vertices.clear();
        let clear_color =
            desktop_scene_vertices(scene, self.size, &mut self.primitive_frame_vertices)
                .unwrap_or(CLEAR_COLOR);
        let primitive_vertex_count = self.primitive_frame_vertices.len();
        upload_primitive_vertices(
            &self.device,
            &self.queue,
            &mut self.primitive_vertex_buffer,
            &mut self.primitive_vertex_capacity,
            &self.primitive_frame_vertices,
        );
        frame_profile.checkpoint("primitive_upload");

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(render_pipeline) = self.render_pipeline.as_ref() {
                render_pass.set_pipeline(render_pipeline);
            }
            if primitive_vertex_count > 0
                && let Some(vertex_buffer) = self.primitive_vertex_buffer.as_ref()
            {
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.draw(0..primitive_vertex_count as u32, 0..1);
            }
        }
        frame_profile.checkpoint("render_pass");

        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        self.first_render_completed = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: "scene",
            smooth_scroll_lines: 0.0,
            text_buffer_count: 0,
            text_area_count: 0,
            primitive_vertices: primitive_vertex_count,
            body_line_count: 0,
            viewport_line_count: 0,
            body_text_window_line_count: 0,
            streaming_text_line_count: 0,
            inline_widget_line_count: 0,
            text_prepared: false,
            primitive_geometry_cache_hit: false,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: scene.metadata.animation_active,
            content_ready: scene.metadata.content_ready,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }

    pub(crate) fn render(
        &mut self,
        app: &DesktopApp,
        monitor_size: Option<PhysicalSize<u32>>,
        smooth_scroll_lines: f32,
        workspace_space_hold_progress: Option<f32>,
    ) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        if !self.boot_frame_presented {
            return self.render_boot_frame();
        }

        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-render-workspace"),
            });
        let now = Instant::now();
        let spinner_tick = desktop_spinner_tick(now);
        frame_profile.checkpoint("frame_setup");

        let scroll_motion_frame = if let DesktopApp::SingleSession(single_session) = app {
            self.single_session_scroll_motion
                .frame(single_session.body_scroll_lines, now)
        } else {
            self.single_session_scroll_motion.clear();
            SingleSessionScrollMotionFrame {
                visual_scroll_lines: 0.0,
                smooth_scroll_lines: 0.0,
                active: false,
            }
        };
        let smooth_scroll_lines = smooth_scroll_lines + scroll_motion_frame.smooth_scroll_lines;
        let mut smooth_scroll_lines = smooth_scroll_lines;
        frame_profile.checkpoint("scroll_motion");

        let (welcome_hero_reveal_progress, welcome_hero_reveal_active) =
            if let DesktopApp::SingleSession(single_session) = app {
                self.welcome_hero_reveal_progress(single_session, now)
            } else {
                self.welcome_hero_reveal_key = None;
                self.welcome_hero_reveal_started_at = None;
                (1.0, false)
            };
        frame_profile.checkpoint("welcome_reveal");

        let (
            workspace_render_layout_for_frame,
            workspace_surface_frames_for_frame,
            workspace_status_color_for_frame,
            workspace_status_text_frame,
        ) = if let DesktopApp::Workspace(workspace) = app {
            let target_layout = workspace_render_layout(workspace, self.size, monitor_size);
            let render_layout = self.viewport_animation.frame(target_layout, now);
            let surface_targets =
                workspace_surface_transition_targets(workspace, self.size, render_layout);
            let surface_frames = WorkspaceSurfaceTransitionFrames::new(
                self.surface_transitions.frame(surface_targets, now),
                self.surface_transitions.is_animating(),
            );
            update_workspace_surface_exit_cache(
                &mut self.workspace_surface_exit_cache,
                workspace,
                &surface_frames,
            );
            let status_color = self
                .status_color_transition
                .frame(workspace_status_bar_target_color(workspace), now);
            let status_text_frame = self
                .status_text_transition
                .frame(workspace_status_text(workspace), now);
            (
                Some(render_layout),
                Some(surface_frames),
                Some(status_color),
                Some(status_text_frame),
            )
        } else {
            self.surface_transitions.clear();
            self.workspace_surface_exit_cache.clear();
            self.status_color_transition.clear();
            self.status_text_transition.clear();
            (None, None, None, None)
        };

        let single_session_issue_layout_for_frame =
            if let DesktopApp::SingleSession(single_session) = app {
                issue_browser_layout(single_session, self.size)
            } else {
                IssueBrowserLayout::hidden(self.size)
            };
        let single_session_render_size = single_session_issue_layout_for_frame.chat_size();

        let mut single_session_rendered_body_key = None;
        let mut workspace_text_panes = Vec::new();
        let mut workspace_text_pane_cache_for_frame = None;
        let mut body_line_count = 0usize;
        let mut viewport_line_count = 0usize;
        let mut body_text_window_line_count = 0usize;
        let mut streaming_text_line_count = 0usize;
        let mut inline_widget_line_count = 0usize;
        let mut streaming_follow_active = false;
        let defer_text_this_frame = self.defer_initial_text_frame;
        if defer_text_this_frame {
            self.defer_initial_text_frame = false;
            self.single_session_text_cache_key = None;
            self.single_session_text_buffers.clear();
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_needs_prepare = false;
            self.single_session_body_text_scroll_start = None;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
        } else if let DesktopApp::SingleSession(single_session) = app {
            let reveal_frame = self
                .streaming_text_reveal
                .frame(&single_session.streaming_response, now);
            self.single_session_streaming_reveal_frame = reveal_frame;
            self.single_session_streaming_revealed_bytes = reveal_frame.revealed_bytes;
            let (rendered_body_key, rendered_body_changed) = self.cached_single_session_body_lines(
                single_session,
                single_session_render_size,
                spinner_tick,
            );
            single_session_rendered_body_key = Some(rendered_body_key);
            body_line_count = self.single_session_body_lines.len();
            // Smoothly follow streaming growth: hold the viewport a fraction of a
            // line above the bottom as new wrapped lines append, then ease down,
            // so the transcript slides instead of snapping a whole line per frame.
            let streaming_follow = self.streaming_follow_motion.frame(
                StreamingFollowInput {
                    total_lines: body_line_count,
                    anchored_to_bottom: single_session.body_scroll_lines
                        <= SCROLL_FRACTIONAL_EPSILON,
                    streaming_active: !single_session.streaming_response.is_empty(),
                },
                now,
            );
            smooth_scroll_lines += streaming_follow.offset_lines;
            streaming_follow_active = streaming_follow.active;
            inline_widget_line_count = single_session.render_inline_widget_visible_line_count();
            frame_profile.checkpoint("body_lines_cache");
            self.ensure_font_system();
            frame_profile.checkpoint("font_system");
            self.refresh_cached_single_session_text_buffers(
                single_session,
                single_session_render_size,
                now,
                smooth_scroll_lines,
                rendered_body_key,
                rendered_body_changed,
            );
            frame_profile.checkpoint("text_buffers_cache");
        } else {
            self.single_session_text_cache_key = None;
            self.single_session_text_key = None;
            self.single_session_text_buffers.clear();
            self.tool_card_motion.clear();
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_reveal.clear();
            self.streaming_follow_motion.clear();
            self.single_session_streaming_reveal_frame = StreamingTextRevealFrame::default();
            self.single_session_streaming_revealed_bytes = 0;
            self.streaming_text_needs_prepare = false;
            self.single_session_body_text_scroll_start = None;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
            if let (DesktopApp::Workspace(workspace), Some(render_layout)) =
                (app, workspace_render_layout_for_frame)
            {
                self.ensure_font_system();
                if let Some(font_system) = self.font_system.as_mut() {
                    let cache = workspace_text_pane_cache_for_frame
                        .get_or_insert_with(|| std::mem::take(&mut self.workspace_text_pane_cache));
                    workspace_text_panes = build_workspace_single_session_text_panes(
                        cache,
                        workspace,
                        self.size,
                        render_layout,
                        workspace_surface_frames_for_frame.as_ref(),
                        font_system,
                    );
                    frame_profile.checkpoint("workspace_text_panes");
                    if !workspace_text_panes.is_empty() {
                        self.text_needs_prepare = true;
                    }
                }
            }
        }
        frame_profile.checkpoint("text_cache");
        if !self.single_session_text_buffers.is_empty() || !workspace_text_panes.is_empty() {
            self.ensure_text_renderer();
        }
        if self.single_session_streaming_text_buffer.is_some() {
            self.ensure_streaming_text_renderer();
        }
        frame_profile.checkpoint("text_renderer");
        self.ensure_render_pipeline();
        frame_profile.checkpoint("primitive_pipeline");
        let streaming_text_arrival_style = self.single_session_streaming_arrival_style(now);
        if let DesktopApp::SingleSession(single_session) = app {
            self.update_single_session_streaming_text_buffer_opacity(
                single_session,
                single_session_render_size,
                streaming_text_arrival_style.opacity,
            );
        }
        if streaming_text_arrival_style.active
            && self.single_session_streaming_text_buffer.is_some()
        {
            self.streaming_text_needs_prepare = true;
        }
        let has_streaming_text_buffer = self.single_session_streaming_text_buffer.is_some();
        self.release_streaming_text_renderer_if_idle(has_streaming_text_buffer);
        let text_buffers = &self.single_session_text_buffers;
        let has_text_buffers = !text_buffers.is_empty() || !workspace_text_panes.is_empty();
        let mut text_area_count = 0usize;
        let mut text_prepared = false;
        let single_session_viewport = if let DesktopApp::SingleSession(single_session) = app {
            let viewport = single_session_body_viewport_from_lines(
                single_session,
                single_session_render_size,
                smooth_scroll_lines,
                &self.single_session_body_lines,
            );
            viewport_line_count = viewport.lines.len();
            body_text_window_line_count = self
                .single_session_body_text_window_start
                .zip(self.single_session_body_text_window_end)
                .map(|(start, end)| end.saturating_sub(start))
                .unwrap_or_default();
            streaming_text_line_count = self
                .single_session_streaming_text_start_line
                .zip(self.single_session_streaming_text_end_line)
                .map(|(start, end)| end.saturating_sub(start))
                .unwrap_or_default();
            Some(viewport)
        } else {
            None
        };
        let welcome_hero_uses_runtime_mask = matches!(
            app,
            DesktopApp::SingleSession(single_session)
                if !single_session_issue_layout_for_frame.visible()
                    && welcome_hero_runtime_mask_supported(&single_session.welcome_hero_text())
        );
        if welcome_hero_reveal_active && !welcome_hero_uses_runtime_mask {
            self.text_needs_prepare = true;
        }
        if self.text_needs_prepare {
            let text_areas = if let DesktopApp::SingleSession(single_session) = app {
                if let Some(viewport) = single_session_viewport.clone() {
                    let areas =
                        single_session_text_areas_for_app_with_cached_body_viewport_and_reveal(
                            single_session,
                            text_buffers,
                            single_session_render_size,
                            smooth_scroll_lines,
                            viewport,
                            welcome_hero_reveal_progress,
                        );
                    if single_session_issue_layout_for_frame.visible() {
                        areas
                            .into_iter()
                            .filter_map(|area| {
                                let area = offset_workspace_text_area(
                                    area,
                                    single_session_issue_layout_for_frame.chat,
                                );
                                (area.bounds.right > area.bounds.left
                                    && area.bounds.bottom > area.bounds.top)
                                    .then_some(area)
                            })
                            .collect()
                    } else {
                        areas
                    }
                } else {
                    desktop_log::error(format_args!(
                        "jcode-desktop: missing single-session viewport while preparing text"
                    ));
                    Vec::new()
                }
            } else if !workspace_text_panes.is_empty() {
                workspace_single_session_text_areas(&workspace_text_panes)
            } else {
                single_session_text_areas(text_buffers, single_session_render_size)
            };
            text_area_count = text_areas.len();
            frame_profile.checkpoint("text_areas");
            if text_areas.is_empty() {
                self.text_needs_prepare = false;
            } else {
                match (
                    self.font_system.as_mut(),
                    self.text_atlas.as_mut(),
                    self.text_renderer.as_mut(),
                ) {
                    (Some(font_system), Some(text_atlas), Some(text_renderer)) => {
                        if let Err(error) = text_renderer.prepare(
                            &self.device,
                            &self.queue,
                            font_system,
                            text_atlas,
                            Resolution {
                                width: self.config.width,
                                height: self.config.height,
                            },
                            text_areas,
                            &mut self.swash_cache,
                        ) {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to prepare text, recreating renderer: {error:?}"
                            ));
                            self.text_renderer = None;
                            self.text_atlas = None;
                            self.text_needs_prepare = true;
                        } else {
                            text_prepared = true;
                            self.text_needs_prepare = false;
                        }
                    }
                    _ => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: text renderer state was incomplete before prepare, recreating renderer"
                        ));
                        self.text_renderer = None;
                        self.text_atlas = None;
                        self.text_needs_prepare = true;
                    }
                }
            }
        } else {
            frame_profile.checkpoint("text_areas");
        }
        frame_profile.checkpoint("text_prepare_static");
        drop(workspace_text_panes);
        if let Some(cache) = workspace_text_pane_cache_for_frame.take() {
            self.workspace_text_pane_cache = cache;
        }
        if self.streaming_text_needs_prepare {
            let streaming_text_areas = if let (
                DesktopApp::SingleSession(single_session),
                Some(viewport),
                Some(buffer),
                Some(start_line),
            ) = (
                app,
                single_session_viewport.clone(),
                self.single_session_streaming_text_buffer.as_ref(),
                self.single_session_streaming_text_start_line,
            ) {
                let area = single_session_streaming_text_area_for_cached_body_viewport(
                    single_session,
                    buffer,
                    single_session_render_size,
                    viewport,
                    start_line,
                    streaming_text_arrival_style.opacity,
                    streaming_text_arrival_style.y_offset_pixels,
                );
                if single_session_issue_layout_for_frame.visible() {
                    let area = offset_workspace_text_area(
                        area,
                        single_session_issue_layout_for_frame.chat,
                    );
                    (area.bounds.right > area.bounds.left && area.bounds.bottom > area.bounds.top)
                        .then_some(area)
                        .into_iter()
                        .collect()
                } else {
                    vec![area]
                }
            } else {
                Vec::new()
            };
            text_area_count += streaming_text_areas.len();
            if streaming_text_areas.is_empty() {
                self.streaming_text_needs_prepare = false;
            } else {
                match (
                    self.font_system.as_mut(),
                    self.streaming_text_atlas.as_mut(),
                    self.streaming_text_renderer.as_mut(),
                ) {
                    (Some(font_system), Some(text_atlas), Some(text_renderer)) => {
                        if let Err(error) = text_renderer.prepare(
                            &self.device,
                            &self.queue,
                            font_system,
                            text_atlas,
                            Resolution {
                                width: self.config.width,
                                height: self.config.height,
                            },
                            streaming_text_areas,
                            &mut self.swash_cache,
                        ) {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to prepare streaming text, recreating renderer: {error:?}"
                            ));
                            self.streaming_text_renderer = None;
                            self.streaming_text_atlas = None;
                            self.streaming_text_needs_prepare = true;
                        } else {
                            text_prepared = true;
                            self.streaming_text_needs_prepare = false;
                        }
                    }
                    _ => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: streaming text renderer state was incomplete before prepare, recreating renderer"
                        ));
                        self.streaming_text_renderer = None;
                        self.streaming_text_atlas = None;
                        self.streaming_text_needs_prepare = true;
                    }
                }
            }
        }
        frame_profile.checkpoint("text_prepare_streaming");

        let mut primitive_geometry_cache_hit = false;
        let (mut vertices, mut animation_active): (Cow<'_, [Vertex]>, bool) = match app {
            DesktopApp::SingleSession(single_session) => {
                let focus_pulse = self.focus_pulse.frame(1, now);
                let inline_selection_motion = self
                    .inline_widget_selection_motion
                    .frame(single_session, now);
                let inline_list_reflow_motion = self
                    .inline_widget_list_reflow_motion
                    .frame(single_session, now);
                let inline_preview_pane_motion = self
                    .inline_widget_preview_pane_motion
                    .frame(single_session, now);
                let composer_motion = self.composer_motion.frame(single_session, now);
                let attachment_chip_motion = self.attachment_chip_motion.frame(single_session, now);
                let stdin_overlay_motion = self.stdin_overlay_motion.frame(
                    single_session,
                    &self.single_session_body_lines,
                    now,
                );
                let tool_motion_lines = single_session_viewport
                    .as_ref()
                    .map(|viewport| viewport.lines.as_slice())
                    .unwrap_or(self.single_session_body_lines.as_slice());
                let transcript_line_height = {
                    let typography =
                        single_session_typography_for_scale(single_session.text_scale());
                    typography.body_size * typography.body_line_height
                };
                let transcript_motion = self.transcript_card_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let transcript_message_motion = self.transcript_message_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let inline_markdown_motion = self.inline_markdown_pill_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let activity_cue_motion = self
                    .streaming_activity_cue_motion
                    .frame(single_session, now);
                let motion_seconds = desktop_pulse_seconds();
                let tool_motion =
                    self.tool_card_motion
                        .frame(tool_motion_lines, now, motion_seconds);
                let scrollbar_motion = self.single_session_scrollbar_motion.frame(
                    single_session,
                    single_session_render_size,
                    self.single_session_body_lines.len(),
                    smooth_scroll_lines,
                    now,
                );
                frame_profile.checkpoint("vertices_tool_motion");
                let animation_active = self.focus_pulse.is_animating()
                    || single_session.has_background_work()
                    || inline_selection_motion.is_active()
                    || inline_list_reflow_motion.is_active()
                    || inline_preview_pane_motion.is_active()
                    || composer_motion.is_active()
                    || attachment_chip_motion.is_active()
                    || stdin_overlay_motion.is_active()
                    || transcript_message_motion.is_active()
                    || transcript_motion.is_active()
                    || inline_markdown_motion.is_active()
                    || activity_cue_motion.is_active()
                    || tool_motion.is_active()
                    || scrollbar_motion.is_active()
                    || scroll_motion_frame.active
                    || welcome_hero_reveal_active
                    || streaming_text_arrival_style.active
                    || self.single_session_streaming_reveal_frame.active
                    || streaming_follow_active;
                let geometry_cache_key = if single_session_issue_layout_for_frame.visible() {
                    None
                } else {
                    single_session_streaming_primitive_geometry_cache_key(
                        single_session,
                        single_session_render_size,
                        focus_pulse,
                        spinner_tick,
                        smooth_scroll_lines,
                        welcome_hero_reveal_progress,
                        tool_motion.cache_key(),
                        inline_list_reflow_motion.cache_key(),
                        inline_preview_pane_motion.cache_key(),
                        composer_motion.cache_key(),
                        attachment_chip_motion.cache_key(),
                        stdin_overlay_motion.cache_key(),
                        transcript_message_motion.cache_key(),
                        transcript_motion.cache_key(),
                        inline_markdown_motion.cache_key(),
                        activity_cue_motion.cache_key(),
                        scrollbar_motion.cache_key(),
                        single_session_rendered_body_key,
                        self.single_session_body_lines.len(),
                    )
                };
                let child_vertices = if let Some(cache_key) = geometry_cache_key {
                    if self.primitive_vertices_cache_key == Some(cache_key) {
                        primitive_geometry_cache_hit = true;
                        Cow::Borrowed(self.primitive_vertices_cache.as_slice())
                    } else {
                        let vertices =
                            build_single_session_vertices_with_cached_body_and_tool_motion(
                                single_session,
                                single_session_render_size,
                                focus_pulse,
                                spinner_tick,
                                motion_seconds,
                                smooth_scroll_lines,
                                welcome_hero_reveal_progress,
                                &self.single_session_body_lines,
                                Some(&inline_selection_motion),
                                Some(&inline_list_reflow_motion),
                                Some(&inline_preview_pane_motion),
                                Some(&composer_motion),
                                Some(&attachment_chip_motion),
                                Some(&stdin_overlay_motion),
                                Some(&transcript_message_motion),
                                Some(&transcript_motion),
                                Some(&inline_markdown_motion),
                                Some(&activity_cue_motion),
                                &tool_motion,
                                Some(&scrollbar_motion),
                            );
                        self.primitive_vertices_cache_key = Some(cache_key);
                        self.primitive_vertices_cache = vertices;
                        Cow::Borrowed(self.primitive_vertices_cache.as_slice())
                    }
                } else {
                    self.primitive_vertices_cache_key = None;
                    Cow::Owned(
                        build_single_session_vertices_with_cached_body_and_tool_motion(
                            single_session,
                            single_session_render_size,
                            focus_pulse,
                            spinner_tick,
                            motion_seconds,
                            smooth_scroll_lines,
                            welcome_hero_reveal_progress,
                            &self.single_session_body_lines,
                            Some(&inline_selection_motion),
                            Some(&inline_list_reflow_motion),
                            Some(&inline_preview_pane_motion),
                            Some(&composer_motion),
                            Some(&attachment_chip_motion),
                            Some(&stdin_overlay_motion),
                            Some(&transcript_message_motion),
                            Some(&transcript_motion),
                            Some(&inline_markdown_motion),
                            Some(&activity_cue_motion),
                            &tool_motion,
                            Some(&scrollbar_motion),
                        ),
                    )
                };
                let vertices = if single_session_issue_layout_for_frame.visible() {
                    Cow::Owned(compose_single_session_issue_browser_vertices(
                        single_session,
                        single_session_issue_layout_for_frame,
                        child_vertices.as_ref(),
                        single_session_render_size,
                        self.size,
                    ))
                } else {
                    child_vertices
                };
                frame_profile.checkpoint("vertices_geometry");
                (vertices, animation_active)
            }
            DesktopApp::Workspace(workspace) => {
                self.inline_widget_selection_motion.clear();
                self.inline_widget_list_reflow_motion.clear();
                self.inline_widget_preview_pane_motion.clear();
                self.composer_motion.clear();
                self.attachment_chip_motion.clear();
                self.stdin_overlay_motion.clear();
                self.transcript_message_motion.clear();
                self.transcript_card_motion.clear();
                self.inline_markdown_pill_motion.clear();
                self.streaming_activity_cue_motion.clear();
                self.single_session_scrollbar_motion.clear();
                self.primitive_vertices_cache_key = None;
                let render_layout = workspace_render_layout_for_frame
                    .unwrap_or_else(|| workspace_render_layout(workspace, self.size, monitor_size));
                let focus_pulse = self.focus_pulse.frame(workspace.focused_id, now);
                let surface_transition_active = workspace_surface_frames_for_frame
                    .as_ref()
                    .is_some_and(|frames| frames.animating);
                let status_text_active = workspace_status_text_frame
                    .as_ref()
                    .is_some_and(StatusTextTransitionFrame::is_active);
                let animation_active = self.viewport_animation.is_animating()
                    || self.focus_pulse.is_animating()
                    || surface_transition_active
                    || self.status_color_transition.is_animating()
                    || status_text_active;
                reserve_workspace_vertex_capacity(
                    &mut self.primitive_workspace_vertices,
                    workspace,
                );
                let status_color = workspace_status_color_for_frame
                    .unwrap_or_else(|| workspace_status_bar_target_color(workspace));
                let status_text_frame = workspace_status_text_frame.as_ref();
                // When nothing is animating, the assembled workspace vertex
                // buffer is a pure function of the workspace content state, so
                // we can reuse the previously built buffer and skip rebuilding
                // ~100k vertices every redraw.
                let workspace_geometry_cache_key = (!animation_active).then(|| {
                    workspace_primitive_vertices_cache_key(
                        workspace,
                        self.size,
                        render_layout,
                        focus_pulse,
                        workspace_space_hold_progress,
                        status_color,
                        status_text_frame,
                        &self.workspace_text_pane_cache,
                    )
                });
                if let Some(cache_key) = workspace_geometry_cache_key {
                    if self.primitive_workspace_vertices_cache_key == Some(cache_key)
                        && !self.primitive_workspace_vertices.is_empty()
                    {
                        primitive_geometry_cache_hit = true;
                        frame_profile.checkpoint("vertices_geometry");
                        (
                            Cow::Borrowed(self.primitive_workspace_vertices.as_slice()),
                            animation_active,
                        )
                    } else {
                        self.primitive_workspace_vertices_cache_key = Some(cache_key);
                        build_vertices_into(
                            WorkspaceVertexBuildParams {
                                workspace,
                                size: self.size,
                                render_layout,
                                focus_pulse,
                                space_hold_progress: workspace_space_hold_progress,
                                surface_frames: workspace_surface_frames_for_frame.as_ref(),
                                exiting_surfaces: &self.workspace_surface_exit_cache,
                                workspace_panel_cache: Some(&self.workspace_text_pane_cache),
                                status_color,
                                status_text_frame,
                            },
                            &mut self.primitive_workspace_vertices,
                        );
                        frame_profile.checkpoint("vertices_geometry");
                        (
                            Cow::Borrowed(self.primitive_workspace_vertices.as_slice()),
                            animation_active,
                        )
                    }
                } else {
                    self.primitive_workspace_vertices_cache_key = None;
                    build_vertices_into(
                        WorkspaceVertexBuildParams {
                            workspace,
                            size: self.size,
                            render_layout,
                            focus_pulse,
                            space_hold_progress: workspace_space_hold_progress,
                            surface_frames: workspace_surface_frames_for_frame.as_ref(),
                            exiting_surfaces: &self.workspace_surface_exit_cache,
                            workspace_panel_cache: Some(&self.workspace_text_pane_cache),
                            status_color,
                            status_text_frame,
                        },
                        &mut self.primitive_workspace_vertices,
                    );
                    frame_profile.checkpoint("vertices_geometry");
                    (
                        Cow::Borrowed(self.primitive_workspace_vertices.as_slice()),
                        animation_active,
                    )
                }
            }
        };
        frame_profile.checkpoint("vertices");
        if let DesktopApp::SingleSession(single_session) = app
            && single_session_caret_visible_for_frame(single_session, spinner_tick)
        {
            if single_session_issue_layout_for_frame.visible() {
                self.primitive_caret_vertices.clear();
                push_single_session_caret(
                    &mut self.primitive_caret_vertices,
                    single_session,
                    single_session_render_size,
                    text_buffers.get(2),
                );
                append_child_vertices_to_parent_with_opacity(
                    vertices.to_mut(),
                    &self.primitive_caret_vertices,
                    single_session_render_size,
                    single_session_issue_layout_for_frame.chat,
                    self.size,
                    1.0,
                );
            } else {
                push_single_session_caret(
                    vertices.to_mut(),
                    single_session,
                    single_session_render_size,
                    text_buffers.get(2),
                );
            }
        }
        frame_profile.checkpoint("caret");
        if let DesktopApp::SingleSession(single_session) = app
            && self.single_session_streaming_text_buffer.is_some()
            && let Some(viewport) = single_session_viewport.as_ref()
        {
            // The streaming tail cursor renders directly into the frame
            // vertices (outside the primitive geometry cache) because it
            // pulses continuously while text streams.
            if !single_session_issue_layout_for_frame.visible() {
                push_single_session_streaming_tail_cursor(
                    vertices.to_mut(),
                    single_session,
                    single_session_render_size,
                    viewport,
                    self.single_session_streaming_text_buffer.as_ref(),
                    self.single_session_streaming_text_start_line,
                    desktop_pulse_seconds(),
                );
                animation_active = true;
            }
        }
        frame_profile.checkpoint("streaming_tail_cursor");
        if let Some(mode_transition_frame) = self.app_mode_transition.frame(app.mode(), now) {
            compose_app_mode_transition_vertices(
                &mut self.app_mode_transition_vertices,
                self.app_mode_transition.previous_vertices(),
                vertices.as_ref(),
                mode_transition_frame,
            );
            vertices = Cow::Borrowed(self.app_mode_transition_vertices.as_slice());
            animation_active = true;
        }
        let uploaded_vertices_snapshot = vertices.as_ref().to_vec();
        self.app_mode_transition
            .remember_uploaded_vertices(&uploaded_vertices_snapshot);
        frame_profile.checkpoint("mode_transition");
        let primitive_vertex_count = vertices.len();
        upload_primitive_vertices(
            &self.device,
            &self.queue,
            &mut self.primitive_vertex_buffer,
            &mut self.primitive_vertex_capacity,
            vertices.as_ref(),
        );
        frame_profile.checkpoint("primitive_upload");

        let welcome_hero_runtime_mask_visible = matches!(
            app,
            DesktopApp::SingleSession(single_session)
                if single_session.is_welcome_timeline_visible() && welcome_hero_uses_runtime_mask
        );
        let defer_hero_mask_this_frame =
            !self.first_render_completed && welcome_hero_runtime_mask_visible;
        let hero_mask_spec = if welcome_hero_runtime_mask_visible
            && !defer_hero_mask_this_frame
            && let DesktopApp::SingleSession(single_session) = app
        {
            welcome_hero_runtime_mask_spec_for_total_lines(
                single_session,
                self.size,
                smooth_scroll_lines,
                self.single_session_body_lines.len(),
            )
        } else {
            None
        };
        if hero_mask_spec.is_some() {
            self.ensure_hero_mask_renderer();
        }
        let hero_mask_prepared = self.hero_mask_renderer.as_mut().is_some_and(|renderer| {
            renderer.prepare(
                &self.device,
                &self.queue,
                self.size,
                hero_mask_spec.as_ref(),
                welcome_hero_reveal_progress,
            )
        });
        frame_profile.checkpoint("hero_mask_prepare");

        let mut text_render_failed = false;
        let mut streaming_text_render_failed = false;
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-workspace-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(render_pipeline) = self.render_pipeline.as_ref() {
                render_pass.set_pipeline(render_pipeline);
            }
            if let Some(vertex_buffer) = self.primitive_vertex_buffer.as_ref() {
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.draw(0..primitive_vertex_count as u32, 0..1);
            }
            if hero_mask_prepared && let Some(hero_mask_renderer) = self.hero_mask_renderer.as_ref()
            {
                hero_mask_renderer.render_prepared(&mut render_pass);
            }
            if has_text_buffers
                && let (Some(text_renderer), Some(text_atlas)) =
                    (self.text_renderer.as_mut(), self.text_atlas.as_ref())
                && let Err(error) = text_renderer.render(text_atlas, &mut render_pass)
            {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to render text, recreating renderer: {error:?}"
                ));
                text_render_failed = true;
            }
            if has_streaming_text_buffer
                && let (Some(text_renderer), Some(text_atlas)) = (
                    self.streaming_text_renderer.as_mut(),
                    self.streaming_text_atlas.as_ref(),
                )
                && let Err(error) = text_renderer.render(text_atlas, &mut render_pass)
            {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to render streaming text, recreating renderer: {error:?}"
                ));
                streaming_text_render_failed = true;
            }
        }
        if text_render_failed {
            self.text_renderer = None;
            self.text_atlas = None;
            self.text_needs_prepare = true;
        }
        if streaming_text_render_failed {
            self.streaming_text_renderer = None;
            self.streaming_text_atlas = None;
            self.streaming_text_needs_prepare = true;
        }
        frame_profile.checkpoint("render_pass");

        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        if welcome_hero_runtime_mask_visible
            && self.welcome_hero_reveal_started_at.is_none()
            && (!welcome_hero_uses_runtime_mask || hero_mask_prepared)
        {
            self.welcome_hero_reveal_started_at = Some(Instant::now());
        }
        self.first_render_completed = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: match app {
                DesktopApp::SingleSession(_) => "single_session",
                DesktopApp::Workspace(_) => "workspace",
            },
            smooth_scroll_lines,
            text_buffer_count: self.single_session_text_buffers.len()
                + usize::from(self.single_session_streaming_text_buffer.is_some()),
            text_area_count,
            primitive_vertices: primitive_vertex_count,
            body_line_count,
            viewport_line_count,
            body_text_window_line_count,
            streaming_text_line_count,
            inline_widget_line_count,
            text_prepared,
            primitive_geometry_cache_hit,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: animation_active
                || defer_text_this_frame
                || defer_hero_mask_this_frame,
            content_ready: text_prepared && !defer_text_this_frame && !defer_hero_mask_this_frame,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }
}

pub(crate) fn upload_primitive_vertices(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    primitive_vertex_buffer: &mut Option<wgpu::Buffer>,
    primitive_vertex_capacity: &mut usize,
    vertices: &[Vertex],
) {
    if vertices.is_empty() {
        return;
    }

    if primitive_vertex_buffer_should_reallocate(*primitive_vertex_capacity, vertices.len()) {
        *primitive_vertex_capacity = primitive_vertex_capacity_for_len(vertices.len());
        let size =
            (*primitive_vertex_capacity * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress;
        *primitive_vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("jcode-desktop-workspace-vertices"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    if let Some(vertex_buffer) = primitive_vertex_buffer.as_ref() {
        queue.write_buffer(vertex_buffer, 0, bytemuck::cast_slice(vertices));
    }
}

pub(crate) fn desktop_scene_vertices(
    scene: &DesktopScene,
    size: PhysicalSize<u32>,
    vertices: &mut Vec<Vertex>,
) -> Option<wgpu::Color> {
    vertices.clear();
    let mut clear_color = None;
    for command in &scene.display_list.commands {
        match command {
            DesktopDisplayCommand::Clear(color) => {
                clear_color = Some(desktop_scene_clear_color(*color));
                vertices.clear();
            }
            DesktopDisplayCommand::Rect(paint) => {
                push_desktop_scene_rect(vertices, paint, size);
            }
            DesktopDisplayCommand::Text(_)
            | DesktopDisplayCommand::Image(_)
            | DesktopDisplayCommand::PushClip(_)
            | DesktopDisplayCommand::PopClip
            | DesktopDisplayCommand::PushLayer { .. }
            | DesktopDisplayCommand::PopLayer => {}
        }
    }
    clear_color
}

pub(crate) fn push_desktop_scene_rect(
    vertices: &mut Vec<Vertex>,
    paint: &DesktopRectPaint,
    size: PhysicalSize<u32>,
) {
    if !paint.rect.is_renderable() || paint.fill.a <= 0.0 {
        return;
    }
    let rect = rect_from_desktop_scene_rect(paint.rect);
    let fill = paint.fill.to_array();
    let radius = [
        paint.radii.top_left,
        paint.radii.top_right,
        paint.radii.bottom_right,
        paint.radii.bottom_left,
    ]
    .into_iter()
    .fold(0.0_f32, f32::max);
    if radius > 0.5 {
        push_rounded_rect(vertices, rect, radius, fill, size);
    } else {
        push_rect(vertices, rect, fill, size);
    }
    if let Some(border) = paint.border
        && border.width > 0.0
        && border.color.a > 0.0
    {
        push_stroked_rect(vertices, rect, border.width, border.color.to_array(), size);
    }
}

pub(crate) fn rect_from_desktop_scene_rect(rect: DesktopSceneRect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

pub(crate) fn desktop_scene_clear_color(color: DesktopColor) -> wgpu::Color {
    wgpu::Color {
        r: color.r as f64,
        g: color.g as f64,
        b: color.b as f64,
        a: color.a as f64,
    }
}

pub(crate) fn primitive_vertex_capacity_for_len(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        len.next_power_of_two()
            .max(PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY)
    }
}

pub(crate) fn primitive_vertex_buffer_should_reallocate(capacity: usize, len: usize) -> bool {
    if len == 0 {
        false
    } else if capacity < len {
        true
    } else {
        capacity > PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY
            && len.saturating_mul(PRIMITIVE_VERTEX_BUFFER_SHRINK_RATIO) < capacity
    }
}

pub(crate) fn streaming_text_renderer_should_release(
    has_streaming_text_buffer: bool,
    renderer_live: bool,
    atlas_live: bool,
) -> bool {
    !has_streaming_text_buffer && (renderer_live || atlas_live)
}

pub(crate) fn single_session_caret_visible_for_frame(
    app: &SingleSessionApp,
    spinner_tick: u64,
) -> bool {
    spinner_tick % 6 < 3 && app.should_draw_composer_caret()
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 2],
    pub(crate) color: [f32; 4],
}

impl Vertex {
    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}
