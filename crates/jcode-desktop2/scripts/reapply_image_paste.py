#!/usr/bin/env python3
"""Idempotently (re)apply desktop2 clipboard-image paste wiring.

Concurrent agents keep rewriting harness.rs/main.rs wholesale, which reverts
small cross-cutting edits. Re-running this restores them whatever shape the
surrounding file is currently in.
"""
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[1] / "src"
changed = []


def edit(name, fn):
    path = root / name
    before = path.read_text()
    after = fn(before)
    if after != before:
        path.write_text(after)
        changed.append(name)


def main_rs(s):
    if "mod clipboard_image;" not in s:
        s = s.replace("mod clipboard;\n", "mod clipboard;\nmod clipboard_image;\n", 1)
    if "mod png;" not in s:
        s = s.replace("mod place;\n", "mod place;\nmod png;\n", 1)
    if "pending_images" not in s:
        s = s.replace(
            "    clipboard: clipboard::Clipboard,\n",
            """    clipboard: clipboard::Clipboard,
    /// Images pasted into the composer, waiting for the next submission.
    ///
    /// Held on `App` rather than in the editor because an attachment is not
    /// text: it has no place in the buffer, it must survive editing the message
    /// written around it, and it is cleared by sending rather than by deleting
    /// a character.
    pending_images: Vec<(String, String)>,
    /// Attachments belonging to messages typed mid-turn and waiting in the
    /// transcript's queue: one entry per queued card, in the same order, so a
    /// message is sent with the images it was written with rather than with
    /// whatever happens to be pending when its turn comes.
    queued_images: std::collections::VecDeque<Vec<(String, String)>>,
""",
            1,
        )
        s = s.replace(
            "            clipboard: clipboard::Clipboard::default(),\n",
            """            clipboard: clipboard::Clipboard::default(),
            pending_images: Vec::new(),
            queued_images: std::collections::VecDeque::new(),
""",
            1,
        )
    if "pub attachments: usize," not in s:
        s = s.replace(
            """    /// Transient one-line notice (e.g. "nothing to undo").
    pub notice: Option<String>,""",
            """    /// Transient one-line notice (e.g. "nothing to undo").
    pub notice: Option<String>,
    /// How many images are attached to the message being written.
    ///
    /// A count on the model rather than the payload: a frame is a pure function
    /// of the model, so the composer can say "1 image attached" in a capture
    /// and in a test without carrying megabytes of base64 through the layout.
    /// The bytes live on `App`, beside the connection that sends them.
    pub attachments: usize,""",
            1,
        )
        s = s.replace("            notice: None,", "            notice: None,\n            attachments: 0,", 1)
    if "images attached" not in s:
        s = s.replace(
            """        if self.scroll > 0.0 {
            return Some("scrolled back".to_string());
        }""",
            """        // Attachments outlive the paste notice: a notice fades, and an image
        // silently attached to a message still being typed is the one thing
        // that must not go invisible before it is sent.
        if self.attachments > 0 {
            return Some(match self.attachments {
                1 => "1 image attached".to_string(),
                count => format!("{count} images attached"),
            });
        }
        if self.scroll > 0.0 {
            return Some("scrolled back".to_string());
        }""",
            1,
        )
    old = """        if self.model.editor.text().trim().is_empty() {
            return;
        }
        if self.model.session_id.is_none() {
            self.model.set_notice("not attached yet");
            return;
        }
        let content = self.model.editor.take_for_submit();"""
    if old in s:
        s = s.replace(
            old,
            """        // An attachment is a message: sending a screenshot with no words is a
        // normal thing to do, so the composer is only empty when there is
        // nothing pending either.
        if self.model.editor.text().trim().is_empty() && self.pending_images.is_empty() {
            return;
        }
        if self.model.session_id.is_none() {
            self.model.set_notice("not attached yet");
            return;
        }
        let mut content = self.model.editor.take_for_submit();
        let images = std::mem::take(&mut self.pending_images);
        self.model.attachments = 0;
        // The transcript card needs something to draw and the daemon needs
        // non-empty content, so an image sent on its own says so rather than
        // appearing as a blank card indistinguishable from a glitch.
        if content.trim().is_empty() {
            content = "[image]".to_string();
        }""",
            1,
        )
    if "self.queued_images.push_back" not in s:
        s = s.replace(
            """        if queued {
            // The turn is still streaming""",
            """        if queued {
            // The attachments wait with their card rather than with the app: the
            // next thing typed gets a fresh set, and this message keeps the
            // images it was written with.
            self.queued_images.push_back(images);
            // The turn is still streaming""",
            1,
        )
    if "self.queued_images.pop_front" not in s:
        s = s.replace(
            """        let Some(content) = self.model.transcript.promote_oldest_queued() else {
            return;
        };
        self.model.busy = true;
        self.model.activity.start(std::time::Instant::now());""",
            """        let Some(content) = self.model.transcript.promote_oldest_queued() else {
            return;
        };
        self.model.busy = true;
        self.model.activity.start(std::time::Instant::now());
        // Oldest first, matching the card being promoted: the queue and this
        // deque are pushed in the same order, so the front is this message's.
        let images = self.queued_images.pop_front().unwrap_or_default();""",
            1,
        )
    old_paste = """            Action::Paste => match self.clipboard.get() {
                Some(text) => self.model.editor.insert_str(&text),
                None => self.model.set_notice("clipboard is empty"),
            },"""
    if old_paste in s:
        s = s.replace(
            old_paste,
            """            // An image on the clipboard outranks text, because a copied image
            // usually also publishes a text flavour (a file URI, or the HTML it
            // came from), and pasting that instead is exactly the bug that made
            // image pasting look broken.
            Action::Paste => match self.clipboard.get_image() {
                Ok(Some(image)) => {
                    let label = image.label();
                    self.pending_images
                        .push((image.media_type, crate::png::base64(&image.bytes)));
                    self.model.attachments = self.pending_images.len();
                    // Said out loud because an attachment is invisible in the
                    // composer text: without this a paste looks like nothing
                    // happened, and a second looks like it replaced the first.
                    self.model.set_notice(match self.pending_images.len() {
                        1 => format!("image attached ({label})"),
                        count => format!("image attached ({label}), {count} total"),
                    });
                }
                Ok(None) => match self.clipboard.get() {
                    Some(text) => self.model.editor.insert_str(&text),
                    None => self.model.set_notice("clipboard is empty"),
                },
                // The clipboard existed but refused: say so rather than paste
                // nothing, which is indistinguishable from the key being
                // ignored.
                Err(error) => self
                    .model
                    .set_notice(format!("clipboard image unavailable: {error}")),
            },""",
            1,
        )
    s = s.replace("harness::Command::Send(content)", "harness::Command::Send { content, images }")
    return s


def clipboard_rs(s):
    if "from_wayland" in s:
        return s
    anchor = "/// Which system buffer an operation refers to."
    s = s.replace(
        anchor,
        """/// An image read from the clipboard, ready to be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Pixel size, when the container declares one. `None` rather than a guess:
    /// the size exists only to tell the user what they attached, and a wrong
    /// number would be worse than saying nothing.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// IANA media type of [`Self::bytes`]. Not always PNG: a JPEG on the
    /// clipboard is forwarded as a JPEG rather than re-encoded, because those
    /// bytes are already smaller than anything this app would produce.
    pub media_type: String,
    /// The encoded image, exactly as it will be sent.
    pub bytes: Vec<u8>,
}

impl Image {
    /// Short description for the caption that tells the user what they
    /// attached: the pixel size when it is known, else the kind.
    pub fn label(&self) -> String {
        match (self.width, self.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => self
                .media_type
                .strip_prefix("image/")
                .unwrap_or("image")
                .to_string(),
        }
    }
}

"""
        + anchor,
        1,
    )
    get = """    /// Read the clipboard, preferring the system and falling back to the last
    /// value set in-process."""
    s = s.replace(
        get,
        """    /// Read an image from the ordinary clipboard.
    ///
    /// The compositor is asked first (see [`crate::clipboard_image`]) because it
    /// hands back the source's own encoded bytes; arboard only offers raw RGBA,
    /// which would have to be re-encoded. `Ok(None)` means "no image on the
    /// clipboard", the ordinary case for a text paste, and must not be reported
    /// to the user as a failure.
    pub fn get_image(&mut self) -> Result<Option<Image>, Unavailable> {
        if self.system && let Some(image) = crate::clipboard_image::from_wayland() {
            let (width, height) = match image.dimensions() {
                Some((width, height)) => (Some(width), Some(height)),
                None => (None, None),
            };
            return Ok(Some(Image {
                width,
                height,
                media_type: image.media_type,
                bytes: image.bytes,
            }));
        }
        let Some(backend) = self.backend() else {
            return Ok(None);
        };
        match backend.get_image() {
            Ok(image) => {
                let width = image.width as u32;
                let height = image.height as u32;
                let bytes = crate::png::encode_rgba(width, height, image.bytes.as_ref());
                Ok(Some(Image {
                    width: Some(width),
                    height: Some(height),
                    media_type: "image/png".to_string(),
                    bytes,
                }))
            }
            // Nothing image-shaped on the clipboard: an absence, not an error.
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(Unavailable(error.to_string())),
        }
    }

"""
        + get,
        1,
    )
    return s


def capture_rs(s):
    if "/// Minimal PNG writer" not in s:
        return s
    return s[: s.index("/// Minimal PNG writer")] + """/// Write tight RGBA8 pixels out as a PNG. Encoding itself lives in `png`, so
/// the clipboard path can produce the same bytes without a file.
fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    std::fs::write(path, crate::png::encode_rgba(width, height, rgba))?;
    Ok(())
}
"""


def harness_rs(s):
    if "Send(String)" in s:
        s = s.replace(
            "    Send(String),",
            """    /// A user message with any images attached to it. The images travel with
    /// the text rather than as a command of their own, so a message and its
    /// attachments can never be split across a reconnect.
    Send {
        content: String,
        images: Vec<(String, String)>,
    },""",
            1,
        )
        s = s.replace("Command::Send(content) => {", "Command::Send { content, images } => {", 1)
    # Two shapes exist depending on which agent last rewrote the worker.
    s = s.replace(
        """                            content,
                            images: vec![],""",
        """                            content,
                            images,""",
        1,
    )
    s = s.replace(
        "client.send_message(&session, &content, vec![], None)",
        "client.send_message(&session, &content, images, None)",
        1,
    )
    return s


CLIPBOARD_IMAGE_PROBE = '''/// `--check-clipboard-image`: prove Ctrl+V's image path against the *real*
/// compositor.
///
/// The unit tests keep the system clipboard sandboxed so they cannot read or
/// clobber a developer's clipboard, which means nothing in the suite exercises
/// Wayland image negotiation at all. That is exactly where pasting a screenshot
/// breaks without a single test failing, so this reads whatever image is on the
/// clipboard right now and reports its type, size, and payload cost.
fn check_clipboard_image() -> Result<()> {
    let mut clipboard = crate::clipboard::Clipboard::system();
    let image = clipboard
        .get_image()
        .map_err(|error| anyhow::anyhow!("clipboard image unavailable: {error}"))?
        .ok_or_else(|| anyhow::anyhow!("clipboard does not contain an image"))?;
    println!(
        "clipboard image ok: {} {}, {} bytes, {} base64 chars",
        image.media_type,
        image.label(),
        image.bytes.len(),
        crate::png::base64(&image.bytes).len()
    );
    Ok(())
}

'''

DISPATCH = '        Some("--check-primary-selection") => Some(check_primary_selection()),'


def cli_rs(s):
    if "check_clipboard_image" not in s and DISPATCH in s:
        s = s.replace(
            DISPATCH,
            '        Some("--check-clipboard-image") => Some(check_clipboard_image()),\n' + DISPATCH,
            1,
        )
        marker = "/// `--check-primary-selection`: prove auto-copy"
        s = s.replace(marker, CLIPBOARD_IMAGE_PROBE + marker, 1)
    return s.replace(
        "harness::Command::Send(message.to_string())",
        """harness::Command::Send {
                    content: message.to_string(),
                    images: vec![],
                }""",
    )


def delivery_tests(s):
    """The queue tests match on the command; the variant gained a field."""
    s = s.replace("Ok(harness::Command::Send(_))", "Ok(harness::Command::Send { .. })")
    return s.replace(
        "Ok(harness::Command::Send(content))",
        "Ok(harness::Command::Send { content, .. })",
    )


def states_rs(s):
    if "attachments" in s:
        return s
    return re.sub(
        r"(\n(\s+)notice: [^\n]*,\n)",
        lambda m: m.group(1) + m.group(2) + "attachments: 0,\n",
        s,
    )


edit("main.rs", main_rs)
edit("clipboard.rs", clipboard_rs)
edit("capture.rs", capture_rs)
edit("harness.rs", harness_rs)
edit("cli.rs", cli_rs)
edit("states.rs", states_rs)
edit("tests/delivery.rs", delivery_tests)
print("reapplied:", ", ".join(changed) if changed else "nothing (already applied)")
sys.exit(0)
