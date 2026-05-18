use std::time::{Instant, Duration};
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

pub struct EnhancedVideoExporter {
    renderer: FrameRenderer,
    encoder: VideoEncoder,
    effects: EffectLibrary,
    config: ExportConfig,
}

pub struct FrameRenderer {
    theme: ExportTheme,
    resolution: Resolution,
    fps: u8,
    font_size: u16,
}

impl FrameRenderer {
    pub fn new(theme: ExportTheme, resolution: Resolution, fps: u8, font_size: u16) -> Self {
        FrameRenderer { theme, resolution, fps, font_size }
    }

    pub fn render_frame(&self, _frame_data: &[Cell]) -> Vec<u8> {
        let w = self.resolution.width as usize;
        let h = self.resolution.height as usize;
        let header_size = 16;
        let mut buf = Vec::with_capacity(header_size + w * h * 4);
        buf.extend_from_slice(&[0x46, 0x52, 0x4D, 0x31]);
        buf.extend_from_slice(&(w as u16).to_le_bytes());
        buf.extend_from_slice(&(h as u16).to_le_bytes());
        buf.resize(buf.capacity(), 0);
        buf
    }
}

#[derive(Clone, Debug)]
pub struct ExportTheme {
    pub background: RgbaColor,
    pub text_primary: RgbaColor,
    pub accent: RgbaColor,
    pub syntax_highlighting: bool,
    pub show_header: bool,
    pub show_timestamps: bool,
    pub show_cursor: bool,
    pub watermark: Option<WatermarkConfig>,
}

impl Default for ExportTheme {
    fn default() -> Self {
        ExportTheme {
            background: RgbaColor { r: 30, g: 30, b: 46, a: 255 },
            text_primary: RgbaColor { r: 220, g: 223, b: 228, a: 255 },
            accent: RgbaColor { r: 97, g: 175, b: 239, a: 255 },
            syntax_highlighting: true,
            show_header: true,
            show_timestamps: false,
            show_cursor: true,
            watermark: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WatermarkConfig {
    pub text: String,
    pub opacity: f64,
    pub position: WatermarkPosition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkPosition { TopLeft, TopRight, BottomLeft, BottomRight, Center }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution { pub width: u16, pub height: u16 }

impl Resolution {
    pub fn new(width: u16, height: u16) -> Self { Resolution { width, height } }
    pub fn area(&self) -> u32 { (self.width as u32) * (self.height as u32) }
    pub fn is_hd(&self) -> bool { self.width >= 1280 && self.height >= 720 }
    pub fn aspect_ratio(&self) -> f64 { self.width as f64 / self.height.max(1) as f64 }
}

pub struct VideoEncoder {
    pub format: OutputFormat,
    pub quality: EncodingQuality,
}

#[derive(Clone, Debug)]
pub enum OutputFormat {
    Gif { optimize: bool, lossy: bool },
    Mp4 { codec: Mp4Codec, crf: Option<u8> },
    Webm { codec: WebmCodec },
    Apng,
    FramesSequence { format: String, prefix: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mp4Codec { H264, H265, VP9, AV1 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebmCodec { VP8, VP9 }

#[derive(Clone, Debug)]
pub struct EncodingQuality {
    pub bitrate_kbps: Option<u32>,
    pub crf: Option<u8>,
    pub colors: ColorDepth,
    pub dithering: DitheringMethod,
}

impl Default for EncodingQuality {
    fn default() -> Self {
        EncodingQuality {
            bitrate_kbps: None,
            crf: Some(20),
            colors: ColorDepth::Rgb24,
            dithering: DitheringMethod::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorDepth { Color256, Rgb24, Rgba32 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DitheringMethod { None, FloydSteinberg, Ordered, Atkinson }

pub struct EffectLibrary {
    transitions: Vec<TransitionEffect>,
    highlights: Vec<HighlightEffect>,
    overlays: Vec<OverlayEffect>,
}

impl EffectLibrary {
    pub fn new() -> Self {
        EffectLibrary {
            transitions: vec![
                TransitionEffect { name: "fade".into(), duration_ms: 500, apply: Self::apply_fade },
                TransitionEffect { name: "slide".into(), duration_ms: 300, apply: Self::apply_slide },
            ],
            highlights: vec![
                HighlightEffect { name: "glow".into(), style: HighlightStyle::Glow, apply: Self::apply_glow },
                HighlightEffect { name: "border".into(), style: HighlightStyle::Border, apply: Self::apply_border },
                HighlightEffect { name: "blink".into(), style: HighlightStyle::Blink, apply: Self::apply_blink },
            ],
            overlays: vec![
                OverlayEffect { name: "timestamp".into(), render: Self::render_timestamp },
                OverlayEffect { name: "label".into(), render: Self::render_label },
            ],
        }
    }

    pub fn get_transition(&self, name: &str) -> Option<&TransitionEffect> {
        self.transitions.iter().find(|t| t.name == name)
    }

    pub fn get_highlight(&self, name: &str) -> Option<&HighlightEffect> {
        self.highlights.iter().find(|h| h.name == name)
    }

    pub fn transition_names(&self) -> Vec<&str> { self.transitions.iter().map(|t| t.name.as_str()).collect() }
    pub fn highlight_names(&self) -> Vec<&str> { self.highlights.iter().map(|h| h.name.as_str()).collect() }

    fn apply_fade(_frame: &mut Frame, _progress: f64) {}
    fn apply_slide(_frame: &mut Frame, _progress: f64) {}
    fn apply_glow(_frame: &mut Frame, _region: &Rect) {}
    fn apply_border(_frame: &mut Frame, _region: &Rect) {}
    fn apply_blink(_frame: &mut Frame, _region: &Rect) {}
    fn render_timestamp(_frame: &mut Frame, _data: &OverlayData) {}
    fn render_label(_frame: &mut Frame, _data: &OverlayData) {}
}

pub struct TransitionEffect {
    pub name: String,
    pub duration_ms: u64,
    pub apply: fn(frame: &mut Frame, progress: f64),
}

pub struct HighlightEffect {
    pub name: String,
    pub style: HighlightStyle,
    pub apply: fn(frame: &mut Frame, region: &Rect),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightStyle { Glow, Border, Background, Underline, Blink }

pub struct OverlayEffect {
    pub name: String,
    pub render: fn(frame: &mut Frame, data: &OverlayData),
}

pub struct OverlayData { pub text: String, pub position: (u16, u16), pub style: Style }

#[derive(Clone, Debug, Default)]
pub struct Style { pub bold: bool, pub italic: bool, pub underline: bool }

pub struct TerminalRecording {
    pub frames: Vec<Frame>,
    pub metadata: RecordingMetadata,
    pub events: Vec<RecordingEvent>,
}

impl TerminalRecording {
    pub fn new(terminal_size: Resolution) -> Self {
        TerminalRecording {
            frames: Vec::new(),
            metadata: RecordingMetadata {
                started_at: Utc::now(),
                ended_at: None,
                terminal_size,
                shell_type: "bash".to_string(),
                command: None,
                session_id: None,
            },
            events: Vec::new(),
        }
    }

    pub fn duration(&self) -> Duration {
        self.frames.last().map_or(Duration::ZERO, |f| f.timestamp)
    }

    pub fn frame_count(&self) -> usize { self.frames.len() }

    pub fn push_frame(&mut self, frame: Frame) { self.frames.push(frame); }

    pub fn push_event(&mut self, event: RecordingEvent) { self.events.push(event); }

    pub fn finish(&mut self) { self.metadata.ended_at = Some(Utc::now()); }
}

pub struct Frame {
    pub timestamp: Duration,
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,
}

impl Frame {
    pub fn new(width: u16, height: u16, timestamp: Duration) -> Self {
        let total = width as usize * height as usize;
        Frame {
            timestamp,
            width,
            height,
            cells: vec![Cell::default(); total],
        }
    }

    pub fn cell_count(&self) -> usize { self.cells.len() }

    pub fn get_cell(&self, x: u16, y: u16) -> Option<&Cell> {
        let idx = y as usize * self.width as usize + x as usize;
        self.cells.get(idx)
    }

    pub fn set_cell(&mut self, x: u16, y: u16, cell: Cell) {
        let idx = y as usize * self.width as usize + x as usize;
        if idx < self.cells.len() { self.cells[idx] = cell; }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub character: char,
    pub foreground: RgbaColor,
    pub background: RgbaColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Cell {
    pub fn new(ch: char, fg: RgbaColor, bg: RgbaColor) -> Self {
        Cell { character: ch, foreground: fg, background: bg, bold: false, italic: false, underline: false }
    }

    pub fn with_style(mut self, bold: bool, italic: bool, underline: bool) -> Self {
        self.bold = bold; self.italic = italic; self.underline = underline; self
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.character == other.character
            && self.foreground == other.foreground
            && self.background == other.background
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
    }
}
impl Eq for Cell {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub terminal_size: Resolution,
    pub shell_type: String,
    pub command: Option<String>,
    pub session_id: Option<Uuid>,
}

use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingEvent {
    pub timestamp: Duration,
    pub event_type: EventType,
    pub data: EventData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventType {
    KeyPress { key: char },
    MouseClick { x: u16, y: u16 },
    CommandStart { command: String },
    CommandEnd { exit_code: i32 },
    ScreenResize { width: u16, height: u16 },
    ClipboardPaste,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventData { Text(String), Position { x: u16, y: u16 }, None }

pub struct MomentMarker {
    pub timestamp: Duration,
    pub label: String,
    pub highlight_style: HighlightStyle,
    pub auto_pause_ms: Option<u64>,
    pub importance: Importance,
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub enum Importance { Normal, Important, Critical }

pub struct ExportConfig {
    pub default_format: OutputFormat,
    pub default_resolution: Resolution,
    pub default_fps: u8,
    pub max_file_size_mb: u64,
    pub enable_watermark: bool,
    pub enable_captions: bool,
    pub enable_cursor_trace: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        ExportConfig {
            default_format: OutputFormat::Gif { optimize: true, lossy: false },
            default_resolution: Resolution::new(800, 600),
            default_fps: 10,
            max_file_size_mb: 50,
            enable_watermark: false,
            enable_captions: false,
            enable_cursor_trace: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportResult {
    pub success: bool,
    pub output_data: Vec<u8>,
    pub format: OutputFormat,
    pub file_size_bytes: usize,
    pub duration_secs: f64,
    pub frame_count: usize,
    pub file_name: String,
}

pub struct HighlightReel {
    pub moments: Vec<MomentMarker>,
    pub padding_secs: f64,
    pub intro_duration: Option<f64>,
    pub outro_duration: Option<f64>,
    pub background_music: Option<String>,
}

impl HighlightReel {
    pub fn new() -> Self {
        HighlightReel {
            moments: Vec::new(),
            padding_secs: 0.5,
            intro_duration: None,
            outro_duration: None,
            background_music: None,
        }
    }

    pub fn add_moment(&mut self, marker: MomentMarker) { self.moments.push(marker); }
    pub fn moment_count(&self) -> usize { self.moments.len() }
    pub fn total_duration(&self) -> f64 {
        if self.moments.is_empty() { return 0.0; }
        let last = self.moments.last().unwrap().timestamp.as_secs_f64();
        let padding = self.padding_secs * self.moments.len().saturating_sub(1) as f64;
        last + padding + self.intro_duration.unwrap_or(0.0) + self.outro_duration.unwrap_or(0.0)
    }
}

pub struct Rect { pub x: u16, pub y: u16, pub width: u16, pub height: u16 }

impl EnhancedVideoExporter {
    pub fn new(config: ExportConfig) -> Self {
        EnhancedVideoExporter {
            renderer: FrameRenderer::new(
                ExportTheme::default(),
                config.default_resolution.clone(),
                config.default_fps,
                14,
            ),
            encoder: VideoEncoder {
                format: config.default_format.clone(),
                quality: EncodingQuality::default(),
            },
            effects: EffectLibrary::new(),
            config,
        }
    }

    pub async fn export_gif(&self, recording: &TerminalRecording, config: &GifExportConfig) -> Result<GifExport, String> {
        let start = Instant::now();
        let mut data = Vec::new();
        let effective_fps = config.fps.min(self.config.default_fps);
        let frame_interval = Duration::from_secs_f64(1.0 / effective_fps as f64);
        let mut exported_frames = 0usize;
        let mut last_export_time = Duration::ZERO;

        for frame in &recording.frames {
            if frame.timestamp >= last_export_time || exported_frames == 0 {
                let rendered = self.renderer.render_frame(&frame.cells);
                data.extend_from_slice(&rendered);
                last_export_time = frame.timestamp + frame_interval;
                exported_frames += 1;
                if let Some(max_dur) = config.max_duration_secs {
                    if frame.timestamp.as_secs() >= max_dur as u64 { break; }
                }
            }
        }

        let elapsed = start.elapsed();
        let data_len = data.len();
        Ok(GifExport {
            data,
            dimensions: (config.width, config.height),
            duration_secs: recording.duration().as_secs_f64(),
            frame_count: exported_frames,
            file_size_bytes: data_len,
            optimized: config.optimize,
        })
    }

    pub async fn export_video(&self, recording: &TerminalRecording, config: &VideoExportConfig) -> Result<VideoExport, String> {
        let start = Instant::now();
        let mut data = Vec::with_capacity(recording.frame_count() * 1024);

        for frame in &recording.frames {
            let rendered = self.renderer.render_frame(&frame.cells);
            data.extend_from_slice(&rendered);
        }

        let elapsed = start.elapsed();
        let data_len = data.len();
        Ok(VideoExport {
            data,
            format: OutputFormat::Mp4 { codec: config.codec.clone(), crf: self.encoder.quality.crf },
            duration_secs: recording.duration().as_secs_f64(),
            file_size_bytes: data_len,
            has_audio: config.include_audio,
        })
    }

    pub async fn create_highlight_reel(&self, recording: &TerminalRecording, reel: &HighlightReel) -> Result<ReelExport, String> {
        let mut included_moments = Vec::new();
        let mut selected_frames = Vec::new();

        for moment in &reel.moments {
            included_moments.push(moment.label.clone());
            for frame in &recording.frames {
                if (frame.timestamp - moment.timestamp).as_secs_f64().abs() < reel.padding_secs / 2.0 {
                    let rendered = self.renderer.render_frame(&frame.cells);
                    selected_frames.extend_from_slice(&rendered);
                }
            }
        }

        Ok(ReelExport {
            data: selected_frames,
            format: self.config.default_format.clone(),
            duration_secs: reel.total_duration(),
            moment_count: reel.moment_count(),
            included_moments,
        })
    }

    pub fn add_caption_overlay(&self, frame: &mut Frame, caption: &str, style: &CaptionStyle) {
        let _ = (caption, style);
        if frame.width > 10 && frame.height > 2 {
            let start_col = ((frame.width as usize).saturating_sub(caption.len().min(frame.width as usize)) / 2) as u16;
            let row = match style.position {
                CaptionPosition::Top => 0,
                CaptionPosition::Bottom => frame.height - 1,
                CaptionPosition::OverlayCenter => frame.height / 2,
            };
            for (i, ch) in caption.chars().enumerate() {
                let x = (start_col as usize + i) as u16;
                if x < frame.width {
                    frame.set_cell(x, row, Cell::new(ch, style.color.clone(), RgbaColor { r: 0, g: 0, b: 0, a: 128 }));
                }
            }
        }
    }

    pub fn add_cursor_trace(&self, frames: &mut [Frame], trace: &CursorTrace) {
        let _ = trace;
        for frame in frames.iter_mut() {
            for &(ref _ts, ref pos) in &trace.positions {
                if pos.line < frame.height && pos.column < frame.width {
                    let cell = frame.get_cell(pos.column, pos.line).cloned().unwrap_or_default();
                    frame.set_cell(pos.column, pos.line, Cell {
                        foreground: trace.color.clone(),
                        background: cell.background,
                        ..cell
                    });
                }
            }
        }
    }

    pub fn add_progress_bar(&self, frame: &mut Frame, progress: f32, message: &str) {
        let bar_width = frame.width.saturating_sub(2) as usize;
        let filled = (progress.clamp(0.0, 1.0) * bar_width as f32) as usize;
        let row = frame.height.saturating_sub(1);

        frame.set_cell(0, row, Cell::new('[', self.config.default_format.accent_color(), RgbaColor::default()));
        for i in 0..bar_width {
            let ch = if i < filled { '=' } else { '-' };
            let color = if i < filled { self.config.default_format.accent_color() } else { RgbaColor { r: 80, g: 80, b: 80, a: 255 } };
            frame.set_cell((i + 1) as u16, row, Cell::new(ch, color, RgbaColor::default()));
        }
        frame.set_cell((bar_width + 1) as u16, row, Cell::new(']', self.config.default_format.accent_color(), RgbaColor::default()));

        let msg_start = message.chars().take(bar_width.saturating_sub(4));
        for (i, ch) in msg_start.enumerate() {
            frame.set_cell(i as u16, row, Cell::new(ch, RgbaColor { r: 200, g: 200, b: 200, a: 255 }, RgbaColor::default()));
        }
    }

    pub fn detect_key_moments(&self, recording: &TerminalRecording) -> Vec<MomentMarker> {
        let mut moments = Vec::new();
        let mut last_cmd_start = None;

        for event in &recording.events {
            match &event.event_type {
                EventType::CommandStart { command } => {
                    last_cmd_start = Some(event.timestamp);
                    moments.push(MomentMarker {
                        timestamp: event.timestamp,
                        label: format!("cmd: {}", command.split_whitespace().next().unwrap_or(command)),
                        highlight_style: HighlightStyle::Glow,
                        auto_pause_ms: Some(500),
                        importance: Importance::Important,
                    });
                }
                EventType::CommandEnd { exit_code } => {
                    if let Some(start) = last_cmd_start {
                        let duration = event.timestamp.saturating_sub(start);
                        if duration.as_secs() > 3 {
                            moments.push(MomentMarker {
                                timestamp: event.timestamp,
                                label: format!("completed ({})", exit_code),
                                highlight_style: if *exit_code == 0 { HighlightStyle::Glow } else { HighlightStyle::Border },
                                auto_pause_ms: Some(300),
                                importance: if *exit_code != 0 { Importance::Critical } else { Importance::Normal },
                            });
                        }
                    }
                    last_cmd_start = None;
                }
                EventType::ScreenResize { .. } => {
                    moments.push(MomentMarker {
                        timestamp: event.timestamp,
                        label: "resize".to_string(),
                        highlight_style: HighlightStyle::Border,
                        auto_pause_ms: None,
                        importance: Importance::Normal,
                    });
                }
                _ => {}
            }
        }
        moments.sort_by_key(|m| m.timestamp);
        moments.dedup_by(|a, b| a.timestamp == b.timestamp);
        moments
    }
}

impl OutputFormat {
    pub fn extension(&self) -> &str {
        match self {
            OutputFormat::Gif { .. } => ".gif",
            OutputFormat::Mp4 { .. } => ".mp4",
            OutputFormat::Webm { .. } => ".webm",
            OutputFormat::Apng => ".apng",
            OutputFormat::FramesSequence { format, .. } => std::str::from_utf8(format.as_bytes()).unwrap_or(".png"),
        }
    }

    pub fn is_animated(&self) -> bool {
        !matches!(self, OutputFormat::FramesSequence { .. })
    }

    pub fn supports_audio(&self) -> bool {
        matches!(self, OutputFormat::Mp4 { .. } | OutputFormat::Webm { .. })
    }

    pub fn accent_color(&self) -> RgbaColor {
        match self {
            OutputFormat::Gif { .. } => RgbaColor { r: 255, g: 100, b: 100, a: 255 },
            OutputFormat::Mp4 { .. } => RgbaColor { r: 100, g: 150, b: 255, a: 255 },
            OutputFormat::Webm { .. } => RgbaColor { r: 100, g: 255, b: 150, a: 255 },
            OutputFormat::Apng => RgbaColor { r: 255, g: 180, b: 100, a: 255 },
            OutputFormat::FramesSequence { .. } => RgbaColor { r: 180, g: 180, b: 180, a: 255 },
        }
    }
}

#[derive(Clone, Debug)]
pub struct GifExportConfig {
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub max_duration_secs: Option<u16>,
    pub quality: GifQuality,
    pub dithering: DitheringMethod,
    pub loop_count: Option<u16>,
    pub optimize: bool,
}

impl Default for GifExportConfig {
    fn default() -> Self {
        GifExportConfig {
            width: 800,
            height: 600,
            fps: 10,
            max_duration_secs: Some(60),
            quality: GifQuality::High,
            dithering: DitheringMethod::None,
            loop_count: Some(0),
            optimize: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GifQuality { Highest, High, Medium, Low }

#[derive(Clone, Debug)]
pub struct GifExport {
    pub data: Vec<u8>,
    pub dimensions: (u16, u16),
    pub duration_secs: f64,
    pub frame_count: usize,
    pub file_size_bytes: usize,
    pub optimized: bool,
}

#[derive(Clone, Debug)]
pub struct VideoExportConfig {
    pub resolution: Resolution,
    pub fps: u8,
    pub codec: Mp4Codec,
    pub quality: EncodingQuality,
    pub include_audio: bool,
}

impl Default for VideoExportConfig {
    fn default() -> Self {
        VideoExportConfig {
            resolution: Resolution::new(1920, 1080),
            fps: 30,
            codec: Mp4Codec::H264,
            quality: EncodingQuality::default(),
            include_audio: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoExport {
    pub data: Vec<u8>,
    pub format: OutputFormat,
    pub duration_secs: f64,
    pub file_size_bytes: usize,
    pub has_audio: bool,
}

#[derive(Clone, Debug)]
pub struct ReelExport {
    pub data: Vec<u8>,
    pub format: OutputFormat,
    pub duration_secs: f64,
    pub moment_count: usize,
    pub included_moments: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CaptionStyle {
    pub font_size: u16,
    pub color: RgbaColor,
    pub background: RgbaColor,
    pub position: CaptionPosition,
    pub animation: CaptionAnimation,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        CaptionStyle {
            font_size: 14,
            color: RgbaColor { r: 255, g: 255, b: 255, a: 255 },
            background: RgbaColor { r: 0, g: 0, b: 0, a: 160 },
            position: CaptionPosition::Bottom,
            animation: CaptionAnimation::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptionPosition { Top, Bottom, OverlayCenter }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptionAnimation { None, FadeIn, Typewriter, Scroll }

pub struct CursorTrace {
    pub positions: Vec<(Duration, Position)>,
    pub color: RgbaColor,
    pub trail_length: usize,
    pub show_clicks: bool,
}

impl CursorTrace {
    pub fn new(color: RgbaColor) -> Self {
        CursorTrace { positions: Vec::new(), color, trail_length: 5, show_clicks: true }
    }

    pub fn record_position(&mut self, ts: Duration, pos: Position) {
        self.positions.push((ts, pos));
        if self.positions.len() > self.trail_length * 20 {
            self.positions.remove(0);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RgbaColor { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Position { pub line: u16, pub column: u16 }

impl Position {
    pub fn new(line: u16, column: u16) -> Self {
        Self { line, column }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_resolution_new_and_properties() {
        let res = Resolution::new(1920, 1080);
        assert_eq!(res.width, 1920);
        assert_eq!(res.height, 1080);
        assert_eq!(res.area(), 1920 * 1080);
        assert!(res.is_hd());
        let ratio = res.aspect_ratio();
        assert!((ratio - 16.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn test_resolution_not_hd() {
        let res = Resolution::new(640, 480);
        assert!(!res.is_hd());
    }

    #[test]
    fn test_resolution_aspect_ratio_zero_height() {
        let res = Resolution::new(100, 0);
        assert_eq!(res.aspect_ratio(), 100.0);
    }

    #[test]
    fn test_rgba_color_default() {
        let c = RgbaColor::default();
        assert_eq!(c.r, 0);
        assert_eq!(c.g, 0);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 0);
    }

    #[test]
    fn test_cell_new_and_with_style() {
        let c = Cell::new('A', RgbaColor { r: 255, g: 0, b: 0, a: 255 }, RgbaColor::default())
            .with_style(true, false, true);
        assert_eq!(c.character, 'A');
        assert!(c.bold);
        assert!(!c.italic);
        assert!(c.underline);
    }

    #[test]
    fn test_cell_equality() {
        let a = Cell::new('X', RgbaColor { r: 1, g: 2, b: 3, a: 255 }, RgbaColor::default());
        let b = Cell::new('X', RgbaColor { r: 1, g: 2, b: 3, a: 255 }, RgbaColor::default());
        assert_eq!(a, b);
        let c = Cell::new('Y', RgbaColor { r: 1, g: 2, b: 3, a: 255 }, RgbaColor::default());
        assert_ne!(a, c);
    }

    #[test]
    fn test_frame_new_and_accessors() {
        let mut frame = Frame::new(10, 5, Duration::from_millis(100));
        assert_eq!(frame.cell_count(), 50);
        assert!(frame.get_cell(0, 0).is_some());
        assert!(frame.get_cell(15, 0).is_none());
        frame.set_cell(3, 2, Cell::new('@', RgbaColor { r: 255, g: 255, b: 0, a: 255 }, RgbaColor::default()));
        let cell = frame.get_cell(3, 2).unwrap();
        assert_eq!(cell.character, '@');
    }

    #[test]
    fn test_terminal_recording_lifecycle() {
        let mut rec = TerminalRecording::new(Resolution::new(80, 24));
        assert_eq!(rec.frame_count(), 0);
        rec.push_frame(Frame::new(80, 24, Duration::ZERO));
        rec.push_frame(Frame::new(80, 24, Duration::from_millis(100)));
        assert_eq!(rec.frame_count(), 2);
        rec.finish();
        assert!(rec.metadata.ended_at.is_some());
    }

    #[test]
    fn test_terminal_recording_duration() {
        let mut rec = TerminalRecording::new(Resolution::new(80, 24));
        rec.push_frame(Frame::new(80, 24, Duration::from_secs(5)));
        rec.push_frame(Frame::new(80, 24, Duration::from_secs(10)));
        assert_eq!(rec.duration().as_secs(), 10);
    }

    #[test]
    fn test_export_config_default() {
        let cfg = ExportConfig::default();
        assert_eq!(cfg.default_fps, 10);
        assert_eq!(cfg.max_file_size_mb, 50);
        assert!(!cfg.enable_watermark);
        assert!(cfg.enable_cursor_trace);
    }

    #[test]
    fn test_output_format_extensions() {
        assert_eq!(OutputFormat::Gif { optimize: true, lossy: false }.extension(), ".gif");
        assert_eq!(OutputFormat::Mp4 { codec: Mp4Codec::H264, crf: None }.extension(), ".mp4");
        assert_eq!(OutputFormat::Webm { codec: WebmCodec::VP9 }.extension(), ".webm");
        assert_eq!(OutputFormat::Apng.extension(), ".apng");
    }

    #[test]
    fn test_output_format_capabilities() {
        let gif = OutputFormat::Gif { optimize: false, lossy: false };
        assert!(gif.is_animated());
        assert!(!gif.supports_audio());

        let mp4 = OutputFormat::Mp4 { codec: Mp4Codec::H264, crf: None };
        assert!(mp4.is_animated());
        assert!(mp4.supports_audio());

        let seq = OutputFormat::FramesSequence { format: "png".into(), prefix: "frame_".into() };
        assert!(!seq.is_animated());
        assert!(!seq.supports_audio());
    }

    #[test]
    fn test_effect_library_transitions() {
        let lib = EffectLibrary::new();
        let names = lib.transition_names();
        assert!(names.contains(&"fade"));
        assert!(names.contains(&"slide"));
        assert!(lib.get_transition("fade").is_some());
        assert!(lib.get_transition("nonexistent").is_none());
    }

    #[test]
    fn test_effect_library_highlights() {
        let lib = EffectLibrary::new();
        let names = lib.highlight_names();
        assert!(names.contains(&"glow"));
        assert!(names.contains(&"border"));
        assert!(names.contains(&"blink"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_highlight_reel_add_and_duration() {
        let mut reel = HighlightReel::new();
        assert_eq!(reel.moment_count(), 0);
        assert_eq!(reel.total_duration(), 0.0);

        reel.add_moment(MomentMarker {
            timestamp: Duration::from_secs(1),
            label: "start".into(),
            highlight_style: HighlightStyle::Glow,
            auto_pause_ms: None,
            importance: Importance::Normal,
        });
        reel.add_moment(MomentMarker {
            timestamp: Duration::from_secs(5),
            label: "end".into(),
            highlight_style: HighlightStyle::Border,
            auto_pause_ms: Some(200),
            importance: Importance::Important,
        });

        assert_eq!(reel.moment_count(), 2);
        assert!(reel.total_duration() > 5.0);
    }

    #[test]
    fn test_enhanced_exporter_creation() {
        let exporter = EnhancedVideoExporter::new(ExportConfig::default());
        assert_eq!(exporter.config.default_fps, 10);
        assert_eq!(exporter.effects.transition_names().len(), 2);
    }

    #[test]
    fn test_caption_style_default() {
        let style = CaptionStyle::default();
        assert_eq!(style.font_size, 14);
        assert_eq!(style.position, CaptionPosition::Bottom);
        assert_eq!(style.animation, CaptionAnimation::None);
    }

    #[test]
    fn test_importance_ordering() {
        assert!(Importance::Critical > Importance::Important);
        assert!(Importance::Important > Importance::Normal);
        assert_eq!(Importance::Normal, Importance::Normal);
    }

    #[test]
    fn test_cursor_trace_record_positions() {
        let mut trace = CursorTrace::new(RgbaColor { r: 255, g: 0, b: 0, a: 255 });
        trace.record_position(Duration::ZERO, Position::new(0, 0));
        trace.record_position(Duration::from_millis(100), Position::new(1, 5));
        assert_eq!(trace.positions.len(), 2);
    }

    #[test]
    fn test_gif_export_config_default() {
        let cfg = GifExportConfig::default();
        assert_eq!(cfg.width, 800);
        assert_eq!(cfg.quality, GifQuality::High);
        assert!(cfg.loop_count == Some(0));
    }

    #[test]
    fn test_video_export_config_default() {
        let cfg = VideoExportConfig::default();
        assert_eq!(cfg.resolution.width, 1920);
        assert_eq!(cfg.codec, Mp4Codec::H264);
        assert!(!cfg.include_audio);
    }

    #[test]
    fn test_export_theme_default() {
        let theme = ExportTheme::default();
        assert!(!theme.show_timestamps);
        assert!(theme.syntax_highlighting);
        assert!(theme.watermark.is_none());
    }
}
