//! Structural validation and resource limits. Hosts call
//! [`validate_document`] on every mounted or patched document before rendering.
use crate::asset::{IMAGE_MIME_TYPES, ImageSource};
use crate::manifest::{Capability, Manifest};
use crate::message::Document;
use crate::view::{Action, Node, NodeKind, host_action};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_depth: usize,
    pub max_nodes: usize,
    /// Total characters of text across the tree.
    pub max_text: usize,
    pub max_inline_image_bytes: usize,
    pub max_asset_bytes: usize,
    pub max_total_asset_bytes: usize,
    pub max_assets: usize,
    pub max_table_cells: usize,
    pub max_html_bytes: usize,
    pub max_state_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 24,
            max_nodes: 5_000,
            max_text: 1_000_000,
            max_inline_image_bytes: 256 * 1024,
            max_asset_bytes: 8 * 1024 * 1024,
            max_total_asset_bytes: 32 * 1024 * 1024,
            max_assets: 256,
            max_table_cells: 20_000,
            max_html_bytes: 256 * 1024,
            max_state_bytes: 256 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    SchemaMismatch(String),
    TooDeep(usize),
    TooManyNodes(usize),
    TooMuchText(usize),
    DuplicateKey(String),
    UnknownAsset(String),
    DuplicateAsset(String),
    AssetTooLarge { id: String, bytes: usize },
    AssetsTooLarge(usize),
    TooManyAssets(usize),
    UnsupportedMime { id: String, mime: String },
    InlineImageTooLarge(usize),
    InvalidImageSource(String),
    MissingCapability(Capability),
    TableTooLarge(usize),
    RaggedTable { expected: usize, got: usize },
    HtmlTooLarge(usize),
    StateNotObject,
    StateTooLarge(usize),
    EmptyAction,
    InvalidValue(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ValidationError {}

/// Validate schema compatibility of a manifest.
pub fn validate_manifest(manifest: &Manifest) -> Result<(), ValidationError> {
    let major = |schema: &str| {
        schema.rsplit_once('/').map(|(name, v)| {
            (
                name.to_owned(),
                v.split('.').next().unwrap_or("").to_owned(),
            )
        })
    };
    if major(&manifest.schema) != major(crate::SCHEMA) {
        return Err(ValidationError::SchemaMismatch(manifest.schema.clone()));
    }
    if manifest.id.trim().is_empty() || manifest.title.trim().is_empty() {
        return Err(ValidationError::InvalidValue(
            "manifest id and title are required".into(),
        ));
    }
    Ok(())
}

/// Validate a document against limits and the applet's declared capabilities.
pub fn validate_document(
    document: &Document,
    manifest: &Manifest,
    limits: &Limits,
) -> Result<(), ValidationError> {
    if !document.state.is_object() {
        return Err(ValidationError::StateNotObject);
    }
    let state_bytes = document.state.to_string().len();
    if state_bytes > limits.max_state_bytes {
        return Err(ValidationError::StateTooLarge(state_bytes));
    }

    if document.assets.len() > limits.max_assets {
        return Err(ValidationError::TooManyAssets(document.assets.len()));
    }
    let mut asset_ids = HashSet::new();
    let mut total = 0usize;
    for asset in &document.assets {
        if !asset_ids.insert(asset.id.as_str()) {
            return Err(ValidationError::DuplicateAsset(asset.id.clone()));
        }
        if !IMAGE_MIME_TYPES.contains(&asset.mime.as_str()) {
            return Err(ValidationError::UnsupportedMime {
                id: asset.id.clone(),
                mime: asset.mime.clone(),
            });
        }
        let bytes = asset.decoded_len();
        if bytes > limits.max_asset_bytes {
            return Err(ValidationError::AssetTooLarge {
                id: asset.id.clone(),
                bytes,
            });
        }
        total += bytes;
    }
    if total > limits.max_total_asset_bytes {
        return Err(ValidationError::AssetsTooLarge(total));
    }

    let mut walk = Walk {
        manifest,
        limits,
        assets: asset_ids,
        nodes: 0,
        text: document.title.len(),
        keys: HashSet::new(),
    };
    walk.node(&document.view, 1)
}

struct Walk<'a> {
    manifest: &'a Manifest,
    limits: &'a Limits,
    assets: HashSet<&'a str>,
    nodes: usize,
    text: usize,
    keys: HashSet<String>,
}

impl Walk<'_> {
    fn require(&self, capability: Capability) -> Result<(), ValidationError> {
        if self.manifest.allows(capability) {
            Ok(())
        } else {
            Err(ValidationError::MissingCapability(capability))
        }
    }

    fn text(&mut self, text: &str) -> Result<(), ValidationError> {
        self.text += text.len();
        if self.text > self.limits.max_text {
            return Err(ValidationError::TooMuchText(self.text));
        }
        Ok(())
    }

    fn action(&self, action: &Action) -> Result<(), ValidationError> {
        if action.action.trim().is_empty() {
            return Err(ValidationError::EmptyAction);
        }
        let capability = match action.action.as_str() {
            host_action::OPEN_URL => Some(Capability::OpenUrl),
            host_action::COPY => Some(Capability::Clipboard),
            host_action::START_CHAT => Some(Capability::StartChat),
            host_action::SEND_PROMPT => Some(Capability::SendPrompt),
            host_action::OPEN_FILE => Some(Capability::ReadFiles),
            host_action::CLOSE | host_action::SET_STATE => None,
            other if other.starts_with("host.") => {
                return Err(ValidationError::InvalidValue(format!(
                    "unknown host action {other}"
                )));
            }
            _ => None,
        };
        if let Some(capability) = capability {
            self.require(capability)?;
        }
        if action.action == host_action::OPEN_URL {
            let url = action
                .args
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !(url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("mailto:"))
            {
                return Err(ValidationError::InvalidValue(format!(
                    "open_url needs an http(s) or mailto url, got {url:?}"
                )));
            }
        }
        Ok(())
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<(), ValidationError> {
        if depth > self.limits.max_depth {
            return Err(ValidationError::TooDeep(depth));
        }
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(ValidationError::TooManyNodes(self.nodes));
        }
        if let Some(key) = &node.key
            && !self.keys.insert(key.clone())
        {
            return Err(ValidationError::DuplicateKey(key.clone()));
        }
        if let Some(fallback) = &node.fallback {
            self.text(fallback)?;
        }
        match &node.kind {
            NodeKind::Text { text, .. }
            | NodeKind::Markdown { text }
            | NodeKind::Code { text, .. } => self.text(text)?,
            NodeKind::Image {
                source,
                alt,
                aspect_ratio,
                ..
            } => {
                self.text(alt)?;
                if let Some(ratio) = aspect_ratio
                    && !(ratio.is_finite() && *ratio > 0.0)
                {
                    return Err(ValidationError::InvalidValue(
                        "aspect_ratio must be positive".into(),
                    ));
                }
                self.image(source)?;
            }
            NodeKind::Table { columns, rows } => {
                let cells = columns.len() * rows.len();
                if cells > self.limits.max_table_cells {
                    return Err(ValidationError::TableTooLarge(cells));
                }
                for row in rows {
                    if row.len() != columns.len() {
                        return Err(ValidationError::RaggedTable {
                            expected: columns.len(),
                            got: row.len(),
                        });
                    }
                    for cell in row {
                        self.text(cell)?;
                    }
                }
            }
            NodeKind::Progress {
                value: Some(value), ..
            } if !(0.0..=1.0).contains(value) => {
                return Err(ValidationError::InvalidValue(
                    "progress must be within 0..=1".into(),
                ));
            }
            NodeKind::Html { source, .. } => {
                self.require(Capability::Html)?;
                if source.len() > self.limits.max_html_bytes {
                    return Err(ValidationError::HtmlTooLarge(source.len()));
                }
            }
            NodeKind::ListItem {
                title, subtitle, ..
            } => {
                self.text(title)?;
                if let Some(subtitle) = subtitle {
                    self.text(subtitle)?;
                }
            }
            _ => {}
        }
        for action in node.actions() {
            self.action(action)?;
        }
        for child in node.children() {
            self.node(child, depth + 1)?;
        }
        Ok(())
    }

    fn image(&self, source: &ImageSource) -> Result<(), ValidationError> {
        match source {
            ImageSource::Asset(id) => {
                if !self.assets.contains(id.as_str()) {
                    return Err(ValidationError::UnknownAsset(id.clone()));
                }
            }
            ImageSource::Data(uri) => {
                let Some(rest) = uri.strip_prefix("data:") else {
                    return Err(ValidationError::InvalidImageSource(
                        "data URI must start with data:".into(),
                    ));
                };
                let Some((mime, payload)) = rest.split_once(";base64,") else {
                    return Err(ValidationError::InvalidImageSource(
                        "data URI must be base64".into(),
                    ));
                };
                if !IMAGE_MIME_TYPES.contains(&mime) {
                    return Err(ValidationError::InvalidImageSource(format!(
                        "unsupported image type {mime}"
                    )));
                }
                let bytes = payload.trim_end_matches('=').len() * 3 / 4;
                if bytes > self.limits.max_inline_image_bytes {
                    return Err(ValidationError::InlineImageTooLarge(bytes));
                }
            }
            ImageSource::Path(path) => {
                self.require(Capability::ReadFiles)?;
                if !std::path::Path::new(path).is_absolute() {
                    return Err(ValidationError::InvalidImageSource(
                        "image path must be absolute".into(),
                    ));
                }
            }
            ImageSource::Url(url) => {
                self.require(Capability::RemoteImages)?;
                if !url.starts_with("https://") {
                    return Err(ValidationError::InvalidImageSource(
                        "remote images must use https".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
