#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub(crate) const DESKTOP_SCENE_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopScene {
    pub(crate) protocol_version: u16,
    pub(crate) viewport: DesktopSceneViewport,
    pub(crate) display_list: DesktopDisplayList,
    pub(crate) metadata: DesktopSceneMetadata,
}

impl DesktopScene {
    pub(crate) fn new(viewport: DesktopSceneViewport) -> Self {
        Self {
            protocol_version: DESKTOP_SCENE_PROTOCOL_VERSION,
            viewport,
            display_list: DesktopDisplayList::default(),
            metadata: DesktopSceneMetadata::default(),
        }
    }

    pub(crate) fn push(&mut self, command: DesktopDisplayCommand) {
        self.display_list.commands.push(command);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.display_list.commands.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSceneViewport {
    pub(crate) size: DesktopSize,
    pub(crate) scale_factor: f32,
}

impl DesktopSceneViewport {
    pub(crate) fn new(width: f32, height: f32, scale_factor: f32) -> Self {
        Self {
            size: DesktopSize { width, height },
            scale_factor,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSceneMetadata {
    pub(crate) title: Option<String>,
    pub(crate) cursor: Option<DesktopCursor>,
    pub(crate) animation_active: bool,
    pub(crate) content_ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopDisplayList {
    pub(crate) commands: Vec<DesktopDisplayCommand>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopDisplayCommand {
    Clear(DesktopColor),
    Rect(DesktopRectPaint),
    Text(DesktopTextBox),
    Image(DesktopImageBox),
    PushClip(DesktopRect),
    PopClip,
    PushLayer { opacity: f32 },
    PopLayer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopPoint {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl DesktopRect {
    pub(crate) fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn is_renderable(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopColor {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl DesktopColor {
    pub(crate) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub(crate) fn from_array(color: [f32; 4]) -> Self {
        Self {
            r: color[0],
            g: color[1],
            b: color[2],
            a: color[3],
        }
    }

    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl Default for DesktopColor {
    fn default() -> Self {
        Self::rgba(0.0, 0.0, 0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopCornerRadii {
    pub(crate) top_left: f32,
    pub(crate) top_right: f32,
    pub(crate) bottom_right: f32,
    pub(crate) bottom_left: f32,
}

impl DesktopCornerRadii {
    pub(crate) fn uniform(radius: f32) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopBorder {
    pub(crate) width: f32,
    pub(crate) color: DesktopColor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopRectPaint {
    pub(crate) rect: DesktopRect,
    pub(crate) fill: DesktopColor,
    pub(crate) radii: DesktopCornerRadii,
    pub(crate) border: Option<DesktopBorder>,
}

impl DesktopRectPaint {
    pub(crate) fn filled(rect: DesktopRect, fill: DesktopColor) -> Self {
        Self {
            rect,
            fill,
            radii: DesktopCornerRadii::default(),
            border: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopTextBox {
    pub(crate) rect: DesktopRect,
    pub(crate) runs: Vec<DesktopTextRun>,
    pub(crate) wrap: DesktopTextWrap,
    pub(crate) align: DesktopTextAlign,
    pub(crate) hit_test: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopTextRun {
    pub(crate) text: String,
    pub(crate) style: DesktopTextStyle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopTextStyle {
    pub(crate) color: DesktopColor,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: f32,
    pub(crate) weight: DesktopFontWeight,
    pub(crate) italic: bool,
}

impl Default for DesktopTextStyle {
    fn default() -> Self {
        Self {
            color: DesktopColor::rgba(0.0, 0.0, 0.0, 1.0),
            font_family: None,
            font_size: 14.0,
            weight: DesktopFontWeight::Regular,
            italic: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopFontWeight {
    Light,
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopTextWrap {
    #[default]
    Word,
    None,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopTextAlign {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopImageBox {
    pub(crate) rect: DesktopRect,
    pub(crate) image: DesktopImageRef,
    pub(crate) opacity: f32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopImageRef {
    InlineRgba { id: String, width: u32, height: u32 },
    Cached { id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopCursor {
    Default,
    Text,
    Pointer,
    Grab,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_defaults_to_current_protocol_version() {
        let scene = DesktopScene::new(DesktopSceneViewport::new(320.0, 240.0, 1.0));
        assert_eq!(scene.protocol_version, DESKTOP_SCENE_PROTOCOL_VERSION);
        assert!(scene.is_empty());
        assert_eq!(scene.viewport.size.width, 320.0);
    }

    #[test]
    fn display_list_preserves_command_order() {
        let mut scene = DesktopScene::new(DesktopSceneViewport::new(100.0, 100.0, 1.0));
        scene.push(DesktopDisplayCommand::Clear(DesktopColor::rgba(
            1.0, 1.0, 1.0, 1.0,
        )));
        scene.push(DesktopDisplayCommand::Rect(DesktopRectPaint::filled(
            DesktopRect::new(1.0, 2.0, 3.0, 4.0),
            DesktopColor::rgba(0.2, 0.3, 0.4, 1.0),
        )));

        assert_eq!(scene.display_list.commands.len(), 2);
        assert!(matches!(
            scene.display_list.commands[0],
            DesktopDisplayCommand::Clear(_)
        ));
        assert!(matches!(
            scene.display_list.commands[1],
            DesktopDisplayCommand::Rect(_)
        ));
    }

    #[test]
    fn rect_renderability_requires_positive_area() {
        assert!(DesktopRect::new(0.0, 0.0, 1.0, 1.0).is_renderable());
        assert!(!DesktopRect::new(0.0, 0.0, 0.0, 1.0).is_renderable());
        assert!(!DesktopRect::new(0.0, 0.0, 1.0, -1.0).is_renderable());
    }
}
