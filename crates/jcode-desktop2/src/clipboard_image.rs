//! Reading an image off the system clipboard.
//!
//! Two paths, in this order, because they are not equivalent:
//!
//! 1. `wl-paste`, when there is a Wayland display. The compositor hands back the
//!    *original* encoded bytes, so a screenshot pasted as PNG stays the small,
//!    compressed PNG the screenshot tool produced.
//! 2. `arboard`, which decodes to raw RGBA and forces us to re-encode. Our PNG
//!    encoder stores scanlines uncompressed, so a 3840x2160 screenshot comes out
//!    around 32 MB, then grows by a third again as base64 on the way to the
//!    daemon. That is the difference between a paste that lands and one that
//!    looks broken, which is why the native path is tried first rather than as a
//!    fallback.
//!
//! `arboard` also reports Wayland images through a data-control protocol that
//! several compositors do not implement, so on those the native path is not an
//! optimization but the only one that works at all.

/// An image read from the clipboard, in whatever container it arrived in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// IANA media type of [`Self::bytes`], e.g. `image/png`.
    pub media_type: String,
    /// The encoded image, exactly as it will be sent.
    pub bytes: Vec<u8>,
}

impl Image {
    /// Pixel size, when the container declares one. Used only to tell the user
    /// what they just attached, so `None` is reported as "image" rather than
    /// guessed at.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        crate::png::dimensions(&self.bytes)
    }
}

/// Media types we can send, in preference order. PNG first: it is what
/// screenshot tools and browsers offer, and it is lossless.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Read an image from the Wayland clipboard via `wl-paste`, or `None` when
/// there is no Wayland display, no `wl-paste`, or no image on the clipboard.
///
/// Offering types is checked before reading so a text-only clipboard costs one
/// cheap call instead of a failed transfer, and so the type we report is the one
/// the source actually published rather than something we inferred.
pub fn from_wayland() -> Option<Image> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return None;
    }
    let listed = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .ok()?;
    if !listed.status.success() {
        return None;
    }
    let offered = String::from_utf8_lossy(&listed.stdout);
    let media_type = IMAGE_TYPES.into_iter().find(|wanted| {
        offered
            .lines()
            .any(|offered| offered.trim().eq_ignore_ascii_case(wanted))
    })?;
    // `--no-newline` matters: wl-paste appends one by default, and a trailing
    // byte past IEND is corruption for some decoders.
    let read = std::process::Command::new("wl-paste")
        .args(["--no-newline", "--type", media_type])
        .output()
        .ok()?;
    if !read.status.success() || read.stdout.is_empty() {
        return None;
    }
    Some(Image {
        media_type: media_type.to_string(),
        bytes: read.stdout,
    })
}

/// Which media type, if any, of those on offer we would take. Split out so the
/// preference order is testable without a compositor: the ordering is the part
/// with a decision in it, and it is not observable from the outside otherwise.
pub fn preferred_type<'a>(offered: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    let offered: Vec<&str> = offered.into_iter().collect();
    IMAGE_TYPES.into_iter().find(|wanted| {
        offered
            .iter()
            .any(|offered| offered.trim().eq_ignore_ascii_case(wanted))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PNG wins when several types are on offer: it is lossless, and it is what
    /// a re-encode would have produced anyway.
    #[test]
    fn png_is_preferred_over_the_lossy_types() {
        assert_eq!(
            preferred_type(["text/html", "image/jpeg", "image/png"]),
            Some("image/png")
        );
    }

    /// A clipboard with no image on it must not be mistaken for one: pasting
    /// text has to keep working, which means saying "no image here".
    #[test]
    fn a_text_only_clipboard_offers_no_image() {
        assert_eq!(
            preferred_type(["text/plain", "text/html", "TEXT", "STRING"]),
            None
        );
    }

    /// Compositors publish types with odd casing and stray whitespace, and
    /// dropping the image because of it would look like paste being broken.
    #[test]
    fn offered_types_are_matched_loosely() {
        assert_eq!(preferred_type([" IMAGE/PNG "]), Some("image/png"));
    }

    /// A JPEG on the clipboard is sent as a JPEG rather than re-encoded: the
    /// bytes are already smaller than anything we would produce.
    #[test]
    fn a_jpeg_only_clipboard_keeps_its_own_container() {
        assert_eq!(preferred_type(["image/jpeg"]), Some("image/jpeg"));
    }

    /// Without a Wayland display there is nothing to ask, and shelling out
    /// anyway would cost a process spawn on every single paste.
    #[test]
    fn no_wayland_display_means_no_native_read() {
        // SAFETY: single-threaded test process, and the value is restored.
        let saved = std::env::var_os("WAYLAND_DISPLAY");
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
        let read = from_wayland();
        if let Some(saved) = saved {
            unsafe { std::env::set_var("WAYLAND_DISPLAY", saved) };
        }
        assert_eq!(read, None);
    }
}
