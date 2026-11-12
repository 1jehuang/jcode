//! Images and other binary assets.
//!
//! Large or reused images should be declared once with [`AssetDecl`] and
//! referenced by id. The host stores them content-addressed, so a patch that
//! changes text never resends image bytes, and assets survive UI reloads.
use serde::{Deserialize, Serialize};

/// Where an image's pixels come from. Every source except `Asset` requires a
/// capability or a size limit, enforced during validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSource {
    /// A previously declared [`AssetDecl`] id. Preferred.
    Asset(String),
    /// A small inline `data:image/...;base64,` URI. Capped by
    /// [`crate::Limits::max_inline_image_bytes`].
    Data(String),
    /// An absolute local file. Requires [`crate::Capability::ReadFiles`].
    Path(String),
    /// An `https://` URL fetched by the host with no cookies or credentials.
    /// Requires [`crate::Capability::RemoteImages`].
    Url(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Letterbox inside the box.
    #[default]
    Contain,
    /// Fill the box, cropping overflow.
    Cover,
    /// Stretch to the box.
    Fill,
}

/// A binary asset. `data` is base64. The host verifies `mime` by sniffing the
/// bytes and rejects mismatches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDecl {
    pub id: String,
    pub mime: String,
    pub data: String,
}

/// Image formats the host decodes. SVG is rendered as static vector art and
/// never executes scripts.
pub const IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
];

impl AssetDecl {
    /// Exact decoded size of `data`, without allocating.
    pub fn decoded_len(&self) -> usize {
        let trimmed = self.data.trim_end_matches('=');
        trimmed.len() * 3 / 4
    }
}
