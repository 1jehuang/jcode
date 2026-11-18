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
    // Fixed-size, content-free frequency history. Four independent rows limit
    // collisions without retaining rejected keys or attachment data.
    frequency: [[u8; 128]; 4],
    observations: usize,
    consecutive_hits: usize,
}

impl ThumbnailCache {
    const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            frequency: [[0; 128]; 4],
            observations: 0,
            consecutive_hits: 0,
        }
    }

    fn observe(&mut self, key: &[u8; 32]) {
        self.observations += 1;
        // Two cache capacities preserve repeated scans but forget stale hot
        // sets promptly. Longer windows measurably penalize phase changes.
        if self.observations == MAX_ENTRIES * 2 {
            for row in &mut self.frequency {
                for count in row {
                    *count /= 2;
                }
            }
            self.observations = 0;
        }
        for (row, counts) in self.frequency.iter_mut().enumerate() {
            let index = key[row * 8] as usize & 127;
            counts[index] = counts[index].saturating_add(1).min(15);
        }
    }

    fn frequency(&self, key: &[u8; 32]) -> u8 {
        (0..4)
            .map(|row| self.frequency[row][key[row * 8] as usize & 127])
            .min()
            .unwrap_or(0)
    }

    // Return the touched entry's new position without copying its image.
    fn touch(&mut self, key: &[u8; 32]) -> Option<usize> {
        let index = self.entries.iter().position(|entry| &entry.0 == key)?;
        self.consecutive_hits = (self.consecutive_hits + 1).min(MAX_ENTRIES * 2);
        self.observe(key);
        let entry = self.entries.remove(index)?;
        self.entries.push_back(entry);
        Some(self.entries.len() - 1)
    }

    fn get(&mut self, key: &[u8; 32]) -> Option<Option<WireImage>> {
        let index = self.touch(key)?;
        Some(self.entries[index].1.clone())
    }

    #[cfg(test)]
    fn insert(&mut self, key: [u8; 32], image: Option<WireImage>) {
        self.insert_borrowed(key, &image);
    }

    fn insert_borrowed(&mut self, key: [u8; 32], image: &Option<WireImage>) {
        let bytes = retained_bytes(image);
        if bytes > MAX_BYTES {
            return;
        }
        // Another caller may have finished the same thumbnail while the cache
        // mutex was released. Keep at most one entry for each content digest.
        if self.touch(&key).is_some() {
            return;
        }
        // A long all-hit run signals a stable working set. Its first miss may
        // begin a new phase, so do not let its old frequency history pin the
        // entire previous set. Cyclic inventories with ongoing misses retain
        // admission protection instead of resetting on every scan.
        if self.consecutive_hits == MAX_ENTRIES * 2 {
            self.frequency = [[0; 128]; 4];
            self.observations = 0;
        }
        self.consecutive_hits = 0;
        self.observe(&key);
        // Admit only if the candidate beats every entry it would displace.
        // Reject ties so a one-pass inventory cannot flush a useful working set.
        // Check before eviction, including when the byte bound needs >1 victim.
        let mut remaining_bytes = self.bytes;
        let mut remaining_entries = self.entries.len();
        for (victim, value) in &self.entries {
            if remaining_entries < MAX_ENTRIES && remaining_bytes + bytes <= MAX_BYTES {
                break;
            }
            if self.frequency(&key) <= self.frequency(victim) {
                return;
            }
            remaining_entries -= 1;
            remaining_bytes -= retained_bytes(value);
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= retained_bytes(&evicted);
        }
        self.bytes += bytes;
        self.entries.push_back((key, image.clone()));
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
        .insert_borrowed(key, &image);
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
#[path = "mobile_sync_thumbnail_cache_clone_tests.rs"]
mod clone_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn image(bytes: usize) -> Option<WireImage> {
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
    fn admission_evicts_lru_only_after_beating_its_frequency() {
        let mut cache = ThumbnailCache::new();
        for n in 0..MAX_ENTRIES {
            cache.insert([n as u8; 32], None);
        }
        assert_eq!(cache.get(&[0; 32]), Some(None));
        cache.insert([255; 32], None);
        assert!(
            cache.get(&[255; 32]).is_none(),
            "one-off tie must be rejected"
        );
        cache.insert([255; 32], None);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.get(&[1; 32]), None);
        assert_eq!(cache.get(&[0; 32]), Some(None));
    }

    #[test]
    fn byte_limit_and_oversized_values_are_bounded() {
        let mut cache = ThumbnailCache::new();
        for n in 0..32 {
            for _ in 0..16 {
                cache.insert([n; 32], image(300_000));
            }
            assert!(cache.bytes <= MAX_BYTES);
        }
        assert_eq!(cache.get(&[0; 32]), None);
        assert!(cache.get(&[31; 32]).is_some());
        let count = cache.entries.len();
        cache.insert([255; 32], image(MAX_BYTES + 1));
        assert_eq!(cache.entries.len(), count);
    }

    #[test]
    fn rejected_candidate_does_not_partially_evict_for_byte_limit() {
        let mut cache = ThumbnailCache::new();
        cache.insert([1; 32], image(1_000_000));
        cache.insert([2; 32], image(1_000_000));
        cache.insert([3; 32], image(1_000_000));
        // Second victim is hotter than the candidate, first victim is colder.
        cache.observe(&[2; 32]);
        cache.observe(&[2; 32]);
        cache.observe(&[4; 32]);
        let before = cache.entries.clone();
        let bytes = cache.bytes;
        cache.insert([4; 32], image(3_000_000));
        assert_eq!(cache.entries, before);
        assert_eq!(cache.bytes, bytes);
        assert!(cache.get(&[4; 32]).is_none());
    }

    #[test]
    fn frequency_history_saturates_and_ages_without_retaining_keys() {
        let mut cache = ThumbnailCache::new();
        for _ in 0..10_000 {
            cache.observe(&[1; 32]);
        }
        assert!(cache.frequency(&[1; 32]) <= 15);
        for _ in 0..MAX_ENTRIES * 2 * 5 {
            cache.observe(&[2; 32]);
        }
        assert_eq!(cache.frequency(&[1; 32]), 0);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn stable_hit_run_resets_stale_history_once_on_new_content() {
        let mut cache = ThumbnailCache::new();
        for n in 0..MAX_ENTRIES {
            cache.insert([n as u8; 32], None);
        }
        for n in 0..10_000 {
            assert_eq!(cache.get(&[(n % MAX_ENTRIES) as u8; 32]), Some(None));
        }
        assert_eq!(cache.consecutive_hits, MAX_ENTRIES * 2);
        cache.insert([64; 32], None);
        assert_eq!(cache.consecutive_hits, 0);
        assert_eq!(cache.observations, 1);
        assert_eq!(cache.frequency(&[64; 32]), 1);
        assert_eq!(cache.frequency(&[1; 32]), 0);
        assert_eq!(cache.get(&[64; 32]), Some(None));
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
    }

    // Exact pre-admission-policy LRU, retained only as a comparison oracle.
    struct Lru(ThumbnailCache);

    impl Lru {
        fn access(&mut self, key: [u8; 32], value: impl FnOnce() -> Option<WireImage>) -> bool {
            if let Some(index) = self.0.entries.iter().position(|entry| entry.0 == key) {
                let entry = self.0.entries.remove(index).unwrap();
                std::hint::black_box(entry.1.clone());
                self.0.entries.push_back(entry);
                return true;
            }
            let value = value();
            let bytes = retained_bytes(&value);
            if bytes <= MAX_BYTES {
                while self.0.entries.len() >= MAX_ENTRIES || self.0.bytes + bytes > MAX_BYTES {
                    let (_, evicted) = self.0.entries.pop_front().unwrap();
                    self.0.bytes -= retained_bytes(&evicted);
                }
                self.0.bytes += bytes;
                self.0.entries.push_back((key, value));
            }
            false
        }
    }

    fn workloads() -> Vec<(&'static str, Vec<usize>)> {
        vec![
            ("fits32", (0..12).flat_map(|_| 0..32).collect()),
            ("cycle40", (0..12).flat_map(|_| 0..40).collect()),
            (
                "hot8_scan64",
                (0..12)
                    .flat_map(|_| (0..4).flat_map(|_| 0..8).chain(8..72))
                    .collect(),
            ),
            (
                "phase_shift",
                (0..2)
                    .flat_map(|phase| (0..32).flat_map(move |_| phase * 32..phase * 32 + 32))
                    .collect(),
            ),
        ]
    }

    fn compare_workload(trace: &[usize], data: Option<&[String]>) -> (usize, usize) {
        let mut cache = ThumbnailCache::new();
        let mut lru = Lru(ThumbnailCache::new());
        let mut hits = [0; 2];
        let mut elapsed = Vec::new();
        for policy in 0..2 {
            let start = std::time::Instant::now();
            for &id in trace {
                let key: [u8; 32] = match data {
                    Some(data) => Sha256::digest(data[id].as_bytes()).into(),
                    None => Sha256::digest(id.to_le_bytes()).into(),
                };
                let value = || data.map_or(None, |data| decode_thumbnail(&data[id]));
                if policy == 0 {
                    hits[0] += usize::from(lru.access(key, value));
                } else if cache.get(&key).is_some() {
                    hits[1] += 1;
                } else {
                    cache.insert(key, value());
                }
                assert!(cache.entries.len() <= MAX_ENTRIES);
                assert!(cache.bytes <= MAX_BYTES);
            }
            elapsed.push(start.elapsed());
        }
        eprintln!(
            "requests={} LRU_hits={} admission_hits={} LRU={:?} admission={:?}",
            trace.len(),
            hits[0],
            hits[1],
            elapsed[0],
            elapsed[1]
        );
        (hits[0], hits[1])
    }

    #[test]
    fn deterministic_working_sets() {
        for (name, trace) in workloads() {
            eprintln!("{name}");
            let (lru, admission) = compare_workload(&trace, None);
            match name {
                "fits32" => assert_eq!(admission, lru),
                "cycle40" | "hot8_scan64" => assert!(admission > lru),
                // A new phase after a stable all-hit run must not add decodes.
                "phase_shift" => assert_eq!(admission, lru),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    #[ignore = "actual image admission benchmark, run with --ignored --nocapture"]
    fn benchmark_admission_working_sets() {
        let data: Vec<_> = (0..72).map(png_data_for).collect();
        // Every hit and miss must produce exactly the uncached image.
        let mut cache = ThumbnailCache::new();
        for (id, data) in data.iter().enumerate() {
            let key = Sha256::digest(id.to_le_bytes()).into();
            let expected = decode_thumbnail(data);
            assert!(expected.is_some());
            cache.insert(key, expected.clone());
            if let Some(actual) = cache.get(&key) {
                assert_eq!(actual, expected);
            }
        }
        for (name, trace) in workloads() {
            eprintln!("actual images {name}");
            compare_workload(&trace, Some(&data));
        }
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
