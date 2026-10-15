# Clipboard Image Paste Fix for jcode

## Problem

When copying an image on macOS and pasting it into jcode with `Cmd+V`, the image was not being pasted. Instead, text (like a file path) was being pasted, or nothing happened at all.

## Root Cause

The `Smart` paste mode in jcode was prioritizing text over images. When you copy an image on macOS, the clipboard often contains both:
- The actual image data
- Text (like a file path or metadata)

The original implementation checked for text first, and if text was found, it would paste that instead of the image. This is the opposite of how Claude Code works.

## Solution

Modified the clipboard paste priority in `crates/jcode-tui/src/tui/app/input.rs` to check for images **first**, then fall back to text if no image is available.

### Changes Made

**File:** `crates/jcode-tui/src/tui/app/input.rs`

**Before:**
```rust
ClipboardPasteKind::Smart => {
    // Check text first
    if let Some(text) = read_text().filter(|t| !t.trim().is_empty()) {
        // ... handle text
    }
    // Only check image if no text found
    if let Some((media_type, base64_data)) = read_image() {
        return image_content(media_type, base64_data);
    }
}
```

**After:**
```rust
ClipboardPasteKind::Smart => {
    // Prioritize images over text to match Claude Code behavior
    if let Some((media_type, base64_data)) = read_image() {
        return image_content(media_type, base64_data);
    }
    // Fall back to text if no image available
    if let Some(text) = read_text().filter(|t| !t.trim().is_empty()) {
        // ... handle text
    }
}
```

### Tests Updated

- Renamed `smart_paste_prefers_normal_text_when_clipboard_has_text` → `smart_paste_prefers_image_when_clipboard_has_both`
- Updated test to expect image content when both image and text are available
- Added new test `smart_paste_uses_text_when_no_image_is_available` to verify text-only paste still works

## How to Use

1. **Copy an image** to clipboard:
   - Take a screenshot: `Cmd+Shift+4` (select area)
   - Copy image from browser/app: `Cmd+C`
   - Copy image file in Finder

2. **Open jcode** (or use existing session)

3. **Paste**: Press `Cmd+V` in the input area

4. **Result**: The image should be attached to your message

## Testing

```bash
# Build the updated version (run from your jcode checkout)
cd <path-to-jcode-repository>
cargo build --release -p jcode --bin jcode

# Run tests
cargo test --package jcode-tui --lib -- tui::app::input::tests --nocapture

# Use the new version
./target/release/jcode
```

## Platform Support

This fix works on all platforms where jcode already supports clipboard images:
- **macOS**: Uses osascript + NSPasteboard (primary) and arboard (fallback)
- **Linux/Wayland**: Uses wl-paste
- **Linux/X11**: Uses arboard

## Version

- Fix applied to: v0.86.2-dev
- Based on commit: ef4c2bd69 (with local modifications)
- Original feature added: v0.66.0 (August 3, 2026)

## Notes

- The `ImageOnly` paste mode already worked correctly (it always prioritized images)
- This fix only affects the default `Smart` paste mode
- Text-only clipboard paste still works when no image is present
- Image URL detection from text still works as a fallback
