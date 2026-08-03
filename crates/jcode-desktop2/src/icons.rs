//! Small, application-owned icon surface backed by vendored Lucide SVG paths.
//!
//! UI code names semantic icons rather than depending on Lucide names. This
//! keeps the source artwork replaceable while giving every icon one renderer,
//! optical size, stroke weight, and line treatment.

use std::sync::OnceLock;

use vello::Scene;
use vello::kurbo::{Affine, BezPath, Rect, Stroke};
use vello::peniko::Color;

const VIEW_BOX_SIDE: f64 = 24.0;
const STROKE_WIDTH: f64 = 2.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    Settings,
    Sessions,
}

impl Icon {
    fn svg(self) -> &'static str {
        match self {
            Self::Settings => include_str!("../assets/icons/settings.svg"),
            Self::Sessions => include_str!("../assets/icons/panels-top-left.svg"),
        }
    }

    fn paths(self) -> &'static [BezPath] {
        static SETTINGS: OnceLock<Vec<BezPath>> = OnceLock::new();
        static SESSIONS: OnceLock<Vec<BezPath>> = OnceLock::new();
        let cache = match self {
            Self::Settings => &SETTINGS,
            Self::Sessions => &SESSIONS,
        };
        cache.get_or_init(|| parse_paths(self.svg())).as_slice()
    }
}

/// Draw an icon centered in `bounds`, preserving its square 24×24 view box.
pub(crate) fn draw(scene: &mut Scene, icon: Icon, bounds: Rect, ink: Color, scale: f64) {
    let side = bounds.width().min(bounds.height());
    let icon_scale = side / VIEW_BOX_SIDE;
    let x = bounds.x0 + (bounds.width() - side) / 2.0;
    let y = bounds.y0 + (bounds.height() - side) / 2.0;
    let transform = Affine::scale(scale) * Affine::translate((x, y)) * Affine::scale(icon_scale);
    let stroke = Stroke::new(STROKE_WIDTH)
        .with_caps(vello::kurbo::Cap::Round)
        .with_join(vello::kurbo::Join::Round);
    for path in icon.paths() {
        scene.stroke(&stroke, transform, ink, None, path);
    }
}

/// Extract path data from trusted, bundled SVG files. Lucide's artwork uses
/// double-quoted `d` attributes, so a full XML dependency would add complexity
/// without accepting any additional input in this closed asset pipeline.
fn parse_paths(svg: &str) -> Vec<BezPath> {
    let mut rest = svg;
    let mut paths = Vec::new();
    while let Some(start) = rest.find(" d=\"") {
        rest = &rest[start + 4..];
        let end = rest
            .find('"')
            .expect("vendored icon has an unterminated path");
        paths.push(BezPath::from_svg(&rest[..end]).expect("vendored icon has invalid path data"));
        rest = &rest[end + 1..];
    }
    assert!(!paths.is_empty(), "vendored icon contains no paths");
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_valid_vendored_paths() {
        for icon in [Icon::Settings, Icon::Sessions] {
            assert!(!icon.paths().is_empty());
            assert!(icon.paths().iter().all(|path| !path.is_empty()));
        }
    }
}
