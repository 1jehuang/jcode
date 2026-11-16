//! Bounded, process-local memoization of attachment thumbnails. Incremental sync
//! checks the complete visible prefix, so unchanged attachments recur on every
//! save. Retain only thumbnails and SHA-256 keys, never original image data.
use super::WireImage;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Mutex;

const MAX_ENTRIES: usize = 32;
const MAX_BYTES: usize = 4 * 1024 * 1024;
static CACHE: Mutex<ThumbnailCache> = Mutex::new(ThumbnailCache::new());

struct ThumbnailCache {
    entries: VecDeque<([u8; 32], Option<WireImage>)>,
    bytes: usize,
}

impl ThumbnailCache {
    const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
        }
    }

    fn get(&mut self, key: &[u8; 32]) -> Option<Option<WireImage>> {
        let index = self.entries.iter().position(|entry| &entry.0 == key)?;
        let entry = self.entries.remove(index)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, key: [u8; 32], image: Option<WireImage>) {
        let bytes = retained_bytes(&image);
        if bytes > MAX_BYTES {
            return;
        }
        // Another caller may have finished the same thumbnail while the cache
        // mutex was released. Keep at most one entry for each content digest.
        if self.get(&key).is_some() {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= retained_bytes(&evicted);
        }
        self.bytes += bytes;
        self.entries.push_back((key, image));
    }
}

fn retained_bytes(image: &Option<WireImage>) -> usize {
    image
        .as_ref()
        .map_or(0, |image| image.data.len() + image.media_type.len())
}

pub(super) fn attachment_thumbnail(data: &str) -> Option<WireImage> {
    let key: [u8; 32] = Sha256::digest(data.as_bytes()).into();
    if let Some(image) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return image;
    }
    // Decoding and encoding can be expensive. Never hold the global cache lock
    // during that work, and memoize invalid attachments as well as valid ones.
    let image = decode_thumbnail(data);
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, image.clone());
    image
}

fn decode_thumbnail(data: &str) -> Option<WireImage> {
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::logging::debug(&format!("Cannot decode mobile attachment base64: {error}"));
            return None;
        }
    };
    if bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    super::image_thumbnail(std::io::Cursor::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(bytes: usize) -> Option<WireImage> {
        Some(WireImage {
            media_type: "image/jpeg".into(),
            data: "x".repeat(bytes),
        })
    }

    #[test]
    fn caches_successes_and_invalid_images() {
        let mut cache = ThumbnailCache::new();
        cache.insert([1; 32], image(12));
        cache.insert([2; 32], None);
        assert_eq!(cache.get(&[1; 32]), Some(image(12)));
        assert_eq!(cache.get(&[2; 32]), Some(None));
        assert_eq!(cache.get(&[3; 32]), None);
        cache.insert([1; 32], image(99));
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.get(&[1; 32]), Some(image(12)));
        assert_eq!(cache.bytes, retained_bytes(&image(12)));
    }

    #[test]
    fn entry_limit_evicts_least_recently_used_including_failures() {
        let mut cache = ThumbnailCache::new();
        for n in 0..MAX_ENTRIES {
            cache.insert([n as u8; 32], None);
        }
        assert_eq!(cache.get(&[0; 32]), Some(None));
        cache.insert([255; 32], None);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.get(&[1; 32]), None);
        assert_eq!(cache.get(&[0; 32]), Some(None));
    }

    #[test]
    fn byte_limit_and_oversized_values_are_bounded() {
        let mut cache = ThumbnailCache::new();
        for n in 0..32 {
            cache.insert([n; 32], image(300_000));
            assert!(cache.bytes <= MAX_BYTES);
        }
        assert_eq!(cache.get(&[0; 32]), None);
        assert!(cache.get(&[31; 32]).is_some());
        let count = cache.entries.len();
        cache.insert([255; 32], image(MAX_BYTES + 1));
        assert_eq!(cache.entries.len(), count);
    }

    fn png_data() -> String {
        png_data_for(0)
    }

    fn png_data_for(seed: u32) -> String {
        use base64::Engine;
        let image = image::RgbImage::from_fn(512, 512, |x, y| {
            image::Rgb([
                ((x + seed) % 251) as u8,
                (y % 241) as u8,
                ((x + y) % 239) as u8,
            ])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    }

    #[test]
    fn cached_thumbnail_matches_uncached_and_content_changes_miss() {
        let data = png_data();
        let expected = decode_thumbnail(&data);
        assert!(expected.is_some());
        assert_eq!(attachment_thumbnail(&data), expected);
        assert_eq!(attachment_thumbnail(&data), expected);
        assert_eq!(attachment_thumbnail("not an image"), None);
        assert_eq!(attachment_thumbnail("not an image"), None);
        let changed = png_data_for(97);
        let changed_expected = decode_thumbnail(&changed);
        assert_ne!(expected, changed_expected);
        assert_eq!(attachment_thumbnail(&changed), changed_expected);
        let blocks = vec![
            super::super::ContentBlock::Image {
                media_type: "image/png".into(),
                data,
            },
            super::super::ContentBlock::Image {
                media_type: "image/png".into(),
                data: changed,
            },
        ];
        assert_eq!(
            super::super::attached_images(&blocks),
            vec![expected.unwrap(), changed_expected.unwrap()]
        );
    }

    #[test]
    #[ignore = "comparative thumbnail CPU benchmark, run with --ignored --nocapture"]
    fn benchmark_repeated_attachment_thumbnails() {
        use std::hint::black_box;
        use std::time::Instant;
        let data = png_data();
        let expected = decode_thumbnail(&data).unwrap();
        assert_eq!(attachment_thumbnail(&data), Some(expected));
        let iterations = 12;
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(decode_thumbnail(black_box(&data)));
        }
        let uncached = start.elapsed();
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(attachment_thumbnail(black_box(&data)));
        }
        let cached = start.elapsed();
        eprintln!(
            "attachment thumbnails: iterations={iterations} uncached={uncached:?} cached={cached:?} ratio={:.2}x",
            uncached.as_secs_f64() / cached.as_secs_f64()
        );
    }
}
