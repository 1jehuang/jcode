//! Screen observation: full-screen + per-window screenshots and OCR.

use super::osa;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use core_graphics::display::CGDisplay;
use jcode_tool_types::ToolOutput;
use serde_json::json;
use std::process::Command;
use std::time::Duration;

/// Read width/height from a PNG IHDR chunk. Returns None if not a PNG.
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != PNG_SIG {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

fn capture_to_temp(extra_args: &[&str]) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!(
        "jcode_computer_{}_{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let tmp_str = tmp.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["-x"];
    args.extend_from_slice(extra_args);
    args.push(&tmp_str);
    let (ok, _out, err) =
        osa::run_command_timed("/usr/sbin/screencapture", &args, Duration::from_secs(15))?;
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        bail!("screencapture failed: {}", err.trim());
    }
    let bytes = std::fs::read(&tmp).context("failed to read screenshot file")?;
    let _ = std::fs::remove_file(&tmp);
    if bytes.is_empty() {
        bail!(
            "screenshot was empty. Grant Screen Recording permission (run the `setup` action), or \
             in System Settings > Privacy & Security > Screen Recording."
        );
    }
    Ok(bytes)
}

pub fn screenshot() -> Result<ToolOutput> {
    // Check screenshot scope
    let scope = std::env::var("JCODE_SCREENSHOT_SCOPE").unwrap_or_default();
    if scope == "browser_only" {
        bail!(
            "Full desktop screenshots are restricted (JCODE_SCREENSHOT_SCOPE=browser_only). \
             Use the browser tool with 'screenshot' action instead."
        );
    }

    let bytes = capture_to_temp(&[])?;
    let bounds = CGDisplay::main().bounds();
    let point_w = bounds.size.width;
    let point_h = bounds.size.height;
    let (pixel_w, pixel_h) = png_dimensions(&bytes).unwrap_or((point_w as u32, point_h as u32));
    let scale = if point_w > 0.0 {
        pixel_w as f64 / point_w
    } else {
        1.0
    };
    let summary = format!(
        "Captured main display: {pixel_w}x{pixel_h} pixels = {point_w:.0}x{point_h:.0} points \
         (scale {scale:.2}x). Click/move coordinates are in POINTS: for a feature at image pixel \
         (px, py), use x = px / {scale:.2}, y = py / {scale:.2}.",
    );
    Ok(ToolOutput::new(summary)
        .with_title("screenshot")
        .with_labeled_image("image/png", STANDARD.encode(&bytes), "screen")
        .with_metadata(json!({
            "width_points": point_w, "height_points": point_h,
            "width_pixels": pixel_w, "height_pixels": pixel_h, "scale": scale,
        })))
}

/// Screenshot a single window by its CoreGraphics window id, even if occluded.
pub fn window_screenshot(window_id: i64) -> Result<ToolOutput> {
    let bytes = capture_to_temp(&["-o", "-l", &window_id.to_string()])?;
    let (pixel_w, pixel_h) = png_dimensions(&bytes).unwrap_or((0, 0));
    Ok(ToolOutput::new(format!(
        "Captured window {window_id}: {pixel_w}x{pixel_h} pixels."
    ))
    .with_title("window screenshot")
    .with_labeled_image("image/png", STANDARD.encode(&bytes), "window")
    .with_metadata(
        json!({ "window_id": window_id, "width_pixels": pixel_w, "height_pixels": pixel_h }),
    ))
}

/// OCR a region (or the whole screen) using the macOS Vision framework via a
/// small inline Swift/JXA bridge. Returns recognized strings with bounding
/// boxes so the model can click located text in apps with no Accessibility.
///
/// Region cropping is done in-process (CoreGraphics) rather than via
/// `screencapture -R`, which is broken on macOS 26.x (it fails with
/// "could not create image from rect"). We always capture the full screen and
/// crop the resulting image, converting the caller's point rect to pixels.
pub fn ocr(region: Option<[f64; 4]>) -> Result<ToolOutput> {
    // Always capture the full screen (region capture via `-R` is unreliable on
    // recent macOS), then crop in the Vision helper.
    let bytes = capture_to_temp(&[])?;

    // Figure out the pixel<->point scale so we can convert the requested region
    // (in points) into pixel coordinates for cropping.
    let bounds = CGDisplay::main().bounds();
    let point_w = bounds.size.width;
    let (pixel_w, pixel_h) = png_dimensions(&bytes).unwrap_or((point_w as u32, 0));
    let scale = if point_w > 0.0 && pixel_w > 0 {
        pixel_w as f64 / point_w
    } else {
        1.0
    };

    // Persist the capture to a temp file for the Swift helper to read.
    let tmp = std::env::temp_dir().join(format!("jcode_ocr_{}.png", std::process::id()));
    std::fs::write(&tmp, &bytes).context("failed to write OCR capture to temp file")?;
    let img_path = tmp.to_string_lossy().to_string();

    // Convert the requested point rect to a pixel crop rect, clamped to the
    // captured image bounds. Origin is top-left in image (pixel) space.
    let crop = region.and_then(|[x, y, w, h]| {
        if w <= 0.0 || h <= 0.0 || pixel_w == 0 || pixel_h == 0 {
            return None;
        }
        let px = (x * scale).max(0.0);
        let py = (y * scale).max(0.0);
        let pw = (w * scale).min(pixel_w as f64 - px).max(0.0);
        let ph = (h * scale).min(pixel_h as f64 - py).max(0.0);
        if pw < 1.0 || ph < 1.0 {
            return None;
        }
        Some([px, py, pw, ph])
    });

    let result = run_vision_ocr(&img_path, crop);
    let _ = std::fs::remove_file(&tmp);
    let text = result?;
    let summary = if text.trim().is_empty() {
        "OCR found no text.".to_string()
    } else {
        text
    };
    Ok(ToolOutput::new(summary)
        .with_title("ocr")
        .with_metadata(json!({
            "scale": scale,
            "captured_width_pixels": pixel_w,
            "captured_height_pixels": pixel_h,
            "region": region,
        })))
}

/// Run the Vision OCR via the `osascript`-launched Swift one-liner is not viable;
/// instead use a tiny Swift program through `swift` if available, falling back to
/// a clear message. Vision has no AppleScript binding, so we shell to Swift.
///
/// `crop` is an optional pixel rect `[x, y, w, h]` (origin top-left) to crop the
/// loaded image before OCR. Cropping in CoreGraphics avoids the broken
/// `screencapture -R` path.
fn run_vision_ocr(image_path: &str, crop: Option<[f64; 4]>) -> Result<String> {
    // Swift literal for the optional crop rect (origin top-left, in pixels).
    let crop_literal = match crop {
        Some([x, y, w, h]) => format!(
            "CGRect(x: {x}, y: {y}, width: {w}, height: {h})",
            x = x as i64,
            y = y as i64,
            w = w as i64,
            h = h as i64
        ),
        None => "nil".to_string(),
    };
    // Prefer a compiled helper if present; otherwise use `swift` to run inline.
    let swift_src = format!(
        r#"
import Foundation
import Vision
import AppKit

let url = URL(fileURLWithPath: "{path}")
guard let img = NSImage(contentsOf: url), var cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {{
    FileHandle.standardError.write("could not load image\n".data(using: .utf8)!)
    exit(2)
}}
let cropRect: CGRect? = {crop}
if let r = cropRect {{
    if let cropped = cg.cropping(to: r) {{
        cg = cropped
    }}
}}
FileHandle.standardError.write("ocr_image_size \(cg.width)x\(cg.height)\n".data(using: .utf8)!)
let req = VNRecognizeTextRequest {{ request, error in
    guard let obs = request.results as? [VNRecognizedTextObservation] else {{ return }}
    for o in obs {{
        guard let top = o.topCandidates(1).first else {{ continue }}
        let b = o.boundingBox // normalized, origin bottom-left
        print("\(b.origin.x),\(b.origin.y),\(b.size.width),\(b.size.height)\t\(top.string)")
    }}
}}
req.recognitionLevel = .accurate
req.usesLanguageCorrection = true
let handler = VNImageRequestHandler(cgImage: cg, options: [:])
try? handler.perform([req])
"#,
        path = image_path,
        crop = crop_literal,
    );

    let swift = which_swift();
    let Some(swift) = swift else {
        bail!(
            "OCR needs the Swift toolchain (Vision framework has no scripting bridge). \
             Install Xcode command line tools: xcode-select --install"
        );
    };

    let out = Command::new(&swift)
        .arg("-")
        .arg(image_path)
        .env("JCODE_OCR_IMG", image_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(swift_src.as_bytes())?;
            }
            child.wait_with_output()
        })
        .context("failed to run swift for OCR")?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("Vision OCR failed: {}", err.trim());
    }
    // The Swift helper reports the (possibly cropped) image size on stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let img_size = stderr
        .lines()
        .find_map(|l| l.strip_prefix("ocr_image_size "))
        .map(str::to_string);
    let raw = String::from_utf8_lossy(&out.stdout);
    // Each line: x,y,w,h\ttext (bbox normalized, origin bottom-left).
    let header = match img_size {
        Some(size) => format!(
            "Recognized text in {size} px image (bbox normalized 0..1, origin bottom-left; \
             multiply by image size):"
        ),
        None => {
            "Recognized text (bbox normalized 0..1, origin bottom-left; multiply by image size):"
                .to_string()
        }
    };
    let mut lines = vec![header];
    for line in raw.lines() {
        lines.push(line.to_string());
    }
    Ok(lines.join("\n"))
}

fn which_swift() -> Option<String> {
    for p in ["/usr/bin/swift", "/usr/local/bin/swift"] {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    // try PATH
    let out = Command::new("/usr/bin/which").arg("swift").output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Used by `osa`-based callers that just need a quick AX-free description.
#[allow(dead_code)]
pub fn frontmost_app() -> Result<String> {
    osa::run_applescript(
        "tell application \"System Events\" to get name of first application process whose frontmost is true",
    )
}
