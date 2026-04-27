use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiNodeRole {
    Screen,
    TextInput,
    Button,
    Banner,
    MessageList,
    Message,
    Composer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiNodeAction {
    Tap,
    SetText,
    TypeText,
    Scroll,
}

pub const DEFAULT_VIEWPORT_WIDTH: i32 = 390;
pub const DEFAULT_VIEWPORT_HEIGHT: i32 = 844;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl UiRect {
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: String,
    pub role: UiNodeRole,
    pub label: String,
    pub value: Option<String>,
    pub visible: bool,
    pub enabled: bool,
    pub focused: bool,
    pub accessibility_label: Option<String>,
    pub accessibility_value: Option<String>,
    pub supported_actions: Vec<UiNodeAction>,
    pub bounds: Option<UiRect>,
    pub children: Vec<UiNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTree {
    pub screen: Screen,
    pub root: UiNode,
}

pub const VISUAL_SCENE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualScene {
    pub schema_version: u32,
    pub viewport: UiRect,
    pub coordinate_space: String,
    pub theme: String,
    pub layers: Vec<VisualLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualLayer {
    pub id: String,
    pub z_index: i32,
    pub primitives: Vec<VisualPrimitive>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VisualPrimitive {
    Rect(VisualRect),
    Text(VisualText),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualRect {
    pub id: String,
    pub semantic_node_id: Option<String>,
    pub bounds: UiRect,
    pub corner_radius: i32,
    pub fill: String,
    pub stroke: Option<String>,
    pub stroke_width: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualText {
    pub id: String,
    pub semantic_node_id: Option<String>,
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub max_width: i32,
    pub font_family: String,
    pub font_size: i32,
    pub font_weight: i32,
    pub line_height: i32,
    pub fill: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotSnapshot {
    pub format: String,
    pub width: i32,
    pub height: i32,
    pub theme: String,
    pub hash: String,
    pub svg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<VisualScene>,
    pub layout: UiTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotDiff {
    pub matches: bool,
    pub expected_hash: String,
    pub actual_hash: String,
    pub expected_len: usize,
    pub actual_len: usize,
    pub first_difference: Option<usize>,
}

pub fn screenshot_snapshot(tree: &UiTree) -> ScreenshotSnapshot {
    let scene = visual_scene(tree);
    let svg = render_scene_svg(&scene);
    let hash = stable_hash_hex(svg.as_bytes());
    ScreenshotSnapshot {
        format: "svg".to_string(),
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
        theme: scene.theme.clone(),
        hash,
        svg,
        scene: Some(scene),
        layout: tree.clone(),
    }
}

pub fn visual_scene(tree: &UiTree) -> VisualScene {
    let viewport = UiRect {
        x: 0,
        y: 0,
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
    };
    let mut layers = vec![VisualLayer {
        id: "background".to_string(),
        z_index: 0,
        primitives: vec![VisualPrimitive::Rect(VisualRect {
            id: "background.canvas".to_string(),
            semantic_node_id: None,
            bounds: viewport,
            corner_radius: 0,
            fill: "#0b1020".to_string(),
            stroke: None,
            stroke_width: 0,
        })],
    }];

    let mut chrome = VisualLayer {
        id: "chrome".to_string(),
        z_index: 10,
        primitives: Vec::new(),
    };
    chrome.primitives.push(VisualPrimitive::Text(VisualText {
        id: "chrome.status.time".to_string(),
        semantic_node_id: None,
        text: "9:41".to_string(),
        x: 28,
        y: 26,
        max_width: 80,
        font_family: "Inter, ui-sans-serif, system-ui".to_string(),
        font_size: 13,
        font_weight: 700,
        line_height: 16,
        fill: "#e5e7eb".to_string(),
    }));
    chrome.primitives.push(VisualPrimitive::Text(VisualText {
        id: "chrome.title".to_string(),
        semantic_node_id: Some(tree.root.id.clone()),
        text: match tree.screen {
            Screen::Onboarding | Screen::Pairing => "Pair jcode".to_string(),
            Screen::Chat => "jcode".to_string(),
        },
        x: 154,
        y: 58,
        max_width: 140,
        font_family: "Inter, ui-sans-serif, system-ui".to_string(),
        font_size: 17,
        font_weight: 800,
        line_height: 22,
        fill: "#f8fafc".to_string(),
    }));
    layers.push(chrome);

    let mut content = VisualLayer {
        id: "content".to_string(),
        z_index: 20,
        primitives: Vec::new(),
    };
    push_visual_node(&mut content.primitives, &tree.root, 0);
    layers.push(content);

    VisualScene {
        schema_version: VISUAL_SCENE_SCHEMA_VERSION,
        viewport,
        coordinate_space: "logical_points_top_left".to_string(),
        theme: "jcode-mobile-rust-scene-v1".to_string(),
        layers,
    }
}

pub fn diff_screenshots(
    expected: &ScreenshotSnapshot,
    actual: &ScreenshotSnapshot,
) -> ScreenshotDiff {
    let first_difference = expected
        .svg
        .as_bytes()
        .iter()
        .zip(actual.svg.as_bytes().iter())
        .position(|(a, b)| a != b)
        .or_else(|| {
            if expected.svg.len() == actual.svg.len() {
                None
            } else {
                Some(expected.svg.len().min(actual.svg.len()))
            }
        });

    ScreenshotDiff {
        matches: expected.hash == actual.hash && first_difference.is_none(),
        expected_hash: expected.hash.clone(),
        actual_hash: actual.hash.clone(),
        expected_len: expected.svg.len(),
        actual_len: actual.svg.len(),
        first_difference,
    }
}

pub fn render_text(tree: &UiTree) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "jcode mobile simulator\nscreen: {:?}\nviewport: {}x{}\n",
        tree.screen, DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT
    ));
    render_text_node(&mut output, &tree.root, 0);
    output
}

fn render_text_node(output: &mut String, node: &UiNode, depth: usize) {
    if !node.visible {
        return;
    }
    let indent = "  ".repeat(depth);
    let bounds = node
        .bounds
        .map(|bounds| {
            format!(
                "@{},{} {}x{}",
                bounds.x, bounds.y, bounds.width, bounds.height
            )
        })
        .unwrap_or_else(|| "@unlaid".to_string());
    let value = node
        .value
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| format!(" = {}", truncate_for_text(value, 72)))
        .unwrap_or_default();
    let actions = if node.supported_actions.is_empty() {
        "-".to_string()
    } else {
        node.supported_actions
            .iter()
            .map(|action| format!("{:?}", action).to_lowercase())
            .collect::<Vec<_>>()
            .join(",")
    };
    output.push_str(&format!(
        "{indent}- {} [{:?}] {bounds} enabled={} actions={} label={}{}\n",
        node.id,
        node.role,
        node.enabled,
        actions,
        truncate_for_text(&node.label, 48),
        value
    ));
    for child in &node.children {
        render_text_node(output, child, depth + 1);
    }
}

fn truncate_for_text(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut output: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    output.push('…');
    output
}

pub fn render_scene_svg(scene: &VisualScene) -> String {
    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        scene.viewport.width, scene.viewport.height, scene.viewport.width, scene.viewport.height
    ));

    let mut layers: Vec<&VisualLayer> = scene.layers.iter().collect();
    layers.sort_by_key(|layer| layer.z_index);
    for layer in layers {
        svg.push_str(&format!(
            r#"<g data-layer="{}" data-z-index="{}">"#,
            xml_escape(&layer.id),
            layer.z_index
        ));
        for primitive in &layer.primitives {
            render_svg_primitive(&mut svg, primitive);
        }
        svg.push_str("</g>");
    }

    svg.push_str("</svg>\n");
    svg
}

fn render_svg_primitive(svg: &mut String, primitive: &VisualPrimitive) {
    match primitive {
        VisualPrimitive::Rect(rect) => {
            let stroke_attrs = rect.stroke.as_ref().map_or_else(String::new, |stroke| {
                format!(
                    r#" stroke="{}" stroke-width="{}""#,
                    xml_escape(stroke),
                    rect.stroke_width
                )
            });
            svg.push_str(&format!(
                r#"<rect data-primitive="{}"{} x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{}"{}/>"#,
                xml_escape(&rect.id),
                data_node_attr(rect.semantic_node_id.as_deref()),
                rect.bounds.x,
                rect.bounds.y,
                rect.bounds.width,
                rect.bounds.height,
                rect.corner_radius,
                xml_escape(&rect.fill),
                stroke_attrs
            ));
        }
        VisualPrimitive::Text(text) => {
            svg.push_str(&format!(
                r#"<text data-primitive="{}"{} x="{}" y="{}" fill="{}" font-family="{}" font-size="{}" font-weight="{}">{}</text>"#,
                xml_escape(&text.id),
                data_node_attr(text.semantic_node_id.as_deref()),
                text.x,
                text.y,
                xml_escape(&text.fill),
                xml_escape(&text.font_family),
                text.font_size,
                text.font_weight,
                xml_escape(&text.text)
            ));
        }
    }
}

fn data_node_attr(node_id: Option<&str>) -> String {
    node_id.map_or_else(String::new, |node_id| {
        format!(r#" data-node="{}""#, xml_escape(node_id))
    })
}

fn push_visual_node(primitives: &mut Vec<VisualPrimitive>, node: &UiNode, depth: usize) {
    if !node.visible {
        return;
    }

    if let Some(bounds) = node.bounds {
        if node.role != UiNodeRole::Screen {
            let style = visual_style_for_node(node);
            primitives.push(VisualPrimitive::Rect(VisualRect {
                id: format!("{}.rect", node.id),
                semantic_node_id: Some(node.id.clone()),
                bounds,
                corner_radius: style.corner_radius,
                fill: style.fill,
                stroke: style.stroke,
                stroke_width: style.stroke_width,
            }));
        }

        if node.role != UiNodeRole::Screen {
            let text = node
                .value
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(&node.label);
            let text_style = visual_text_style_for_node(node);
            primitives.push(VisualPrimitive::Text(VisualText {
                id: format!("{}.label", node.id),
                semantic_node_id: Some(node.id.clone()),
                text: truncate_for_svg(text, 54usize.saturating_sub(depth * 4)),
                x: bounds.x + text_style.inset_x,
                y: bounds.y + text_style.baseline_y,
                max_width: bounds.width - text_style.inset_x * 2,
                font_family: "Inter, ui-sans-serif, system-ui".to_string(),
                font_size: text_style.font_size,
                font_weight: text_style.font_weight,
                line_height: text_style.line_height,
                fill: text_style.fill,
            }));
        }
    }

    for child in &node.children {
        push_visual_node(primitives, child, depth + 1);
    }
}

struct VisualNodeStyle {
    fill: String,
    stroke: Option<String>,
    stroke_width: i32,
    corner_radius: i32,
}

struct VisualTextStyle {
    fill: String,
    font_size: i32,
    font_weight: i32,
    line_height: i32,
    inset_x: i32,
    baseline_y: i32,
}

fn visual_style_for_node(node: &UiNode) -> VisualNodeStyle {
    let (fill, stroke, stroke_width, corner_radius) = match node.role {
        UiNodeRole::Screen => ("#00000000", None, 0, 0),
        UiNodeRole::TextInput | UiNodeRole::Composer => ("#111827", Some("#334155"), 1, 16),
        UiNodeRole::Button => {
            if node.enabled {
                ("#2563eb", Some("#60a5fa"), 1, 16)
            } else {
                ("#334155", Some("#475569"), 1, 16)
            }
        }
        UiNodeRole::Banner if node.id == "banner.error" => ("#3f1d2b", Some("#fb7185"), 1, 14),
        UiNodeRole::Banner => ("#082f49", Some("#38bdf8"), 1, 14),
        UiNodeRole::MessageList => ("#0f172a", Some("#1e293b"), 1, 18),
        UiNodeRole::Message if node.label.starts_with("User") => {
            ("#1d4ed8", Some("#60a5fa"), 1, 18)
        }
        UiNodeRole::Message if node.label.starts_with("System") => {
            ("#3f1d2b", Some("#fb7185"), 1, 18)
        }
        UiNodeRole::Message => ("#111827", Some("#334155"), 1, 18),
    };
    VisualNodeStyle {
        fill: fill.to_string(),
        stroke: stroke.map(str::to_string),
        stroke_width,
        corner_radius,
    }
}

fn visual_text_style_for_node(node: &UiNode) -> VisualTextStyle {
    let button_text = node.role == UiNodeRole::Button && node.enabled;
    let fill = match node.role {
        UiNodeRole::Button if button_text => "#ffffff",
        UiNodeRole::Button => "#cbd5e1",
        UiNodeRole::Message if node.label.starts_with("User") => "#eff6ff",
        UiNodeRole::Banner if node.id == "banner.error" => "#ffe4e6",
        UiNodeRole::Banner => "#e0f2fe",
        _ => "#e5e7eb",
    };
    let (font_size, font_weight, line_height, baseline_y) = match node.role {
        UiNodeRole::Button => (15, 800, 20, 28),
        UiNodeRole::Banner => (13, 700, 18, 28),
        UiNodeRole::Message => (14, 500, 20, 32),
        UiNodeRole::TextInput | UiNodeRole::Composer => (15, 500, 20, 32),
        UiNodeRole::MessageList | UiNodeRole::Screen => (14, 600, 20, 28),
    };
    VisualTextStyle {
        fill: fill.to_string(),
        font_size,
        font_weight,
        line_height,
        inset_x: 14,
        baseline_y,
    }
}

fn truncate_for_svg(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut output: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    output.push('…');
    output
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub fn hit_test(tree: &UiTree, x: i32, y: i32) -> Option<&UiNode> {
    hit_test_node(&tree.root, x, y)
}

pub fn hit_test_actionable(tree: &UiTree, x: i32, y: i32, action: UiNodeAction) -> Option<&UiNode> {
    hit_test_actionable_node(&tree.root, x, y, action)
}

fn hit_test_node(node: &UiNode, x: i32, y: i32) -> Option<&UiNode> {
    if !node.visible
        || !node
            .bounds
            .is_some_and(|bounds| bounds.contains_point(x, y))
    {
        return None;
    }

    node.children
        .iter()
        .rev()
        .find_map(|child| hit_test_node(child, x, y))
        .or(Some(node))
}

fn hit_test_actionable_node(
    node: &UiNode,
    x: i32,
    y: i32,
    action: UiNodeAction,
) -> Option<&UiNode> {
    if !node.visible
        || !node
            .bounds
            .is_some_and(|bounds| bounds.contains_point(x, y))
    {
        return None;
    }

    node.children
        .iter()
        .rev()
        .find_map(|child| hit_test_actionable_node(child, x, y, action))
        .or_else(|| {
            if node.enabled && node.supported_actions.contains(&action) {
                Some(node)
            } else {
                None
            }
        })
}
