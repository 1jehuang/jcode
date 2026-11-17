//! Frozen pre-optimization operations, plus exact-state differential coverage.
use super::*;

fn old_get(cache: &mut ThumbnailCache, key: &[u8; 32]) -> Option<Option<WireImage>> {
    let index = cache.entries.iter().position(|entry| &entry.0 == key)?;
    cache.consecutive_hits = (cache.consecutive_hits + 1).min(MAX_ENTRIES * 2);
    cache.observe(key);
    let entry = cache.entries.remove(index)?;
    let image = entry.1.clone();
    cache.entries.push_back(entry);
    Some(image)
}

fn old_insert(cache: &mut ThumbnailCache, key: [u8; 32], image: Option<WireImage>) {
    let bytes = retained_bytes(&image);
    if bytes > MAX_BYTES {
        return;
    }
    if old_get(cache, &key).is_some() {
        return;
    }
    if cache.consecutive_hits == MAX_ENTRIES * 2 {
        cache.frequency = [[0; 128]; 4];
        cache.observations = 0;
    }
    cache.consecutive_hits = 0;
    cache.observe(&key);
    let mut remaining_bytes = cache.bytes;
    let mut remaining_entries = cache.entries.len();
    for (victim, value) in &cache.entries {
        if remaining_entries < MAX_ENTRIES && remaining_bytes + bytes <= MAX_BYTES {
            break;
        }
        if cache.frequency(&key) <= cache.frequency(victim) {
            return;
        }
        remaining_entries -= 1;
        remaining_bytes -= retained_bytes(value);
    }
    while cache.entries.len() >= MAX_ENTRIES || cache.bytes + bytes > MAX_BYTES {
        let Some((_, evicted)) = cache.entries.pop_front() else {
            break;
        };
        cache.bytes -= retained_bytes(&evicted);
    }
    cache.bytes += bytes;
    cache.entries.push_back((key, image));
}

fn assert_state(old: &ThumbnailCache, new: &ThumbnailCache) {
    assert_eq!(old.entries, new.entries, "values, admission and LRU");
    assert_eq!(old.bytes, new.bytes);
    assert_eq!(old.frequency, new.frequency);
    assert_eq!(old.observations, new.observations);
    assert_eq!(old.consecutive_hits, new.consecutive_hits);
}

#[test]
fn differential_exact_policy_and_outputs() {
    let mut old = ThumbnailCache::new();
    let mut new = ThumbnailCache::new();
    let values = [
        None,
        tests::image(7),
        tests::image(300_000),
        tests::image(2_000_000),
        tests::image(MAX_BYTES + 1),
    ];
    let mut seed = 0x1234_5678_u64;
    for step in 0..12_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = Sha256::digest((seed >> 32 & 63).to_le_bytes()).into();
        if step % 3 == 0 {
            assert_eq!(old_get(&mut old, &key), new.get(&key));
        } else {
            let image = &values[(seed as usize >> 8) % values.len()];
            old_insert(&mut old, key, image.clone());
            new.insert_borrowed(key, image);
        }
        assert_state(&old, &new);
    }
    // Exercise the all-hit reset boundary and oversize-before-duplicate order.
    let mut old = ThumbnailCache::new();
    let mut new = ThumbnailCache::new();
    for n in 0..MAX_ENTRIES {
        old_insert(&mut old, [n as u8; 32], None);
        new.insert_borrowed([n as u8; 32], &None);
    }
    for _ in 0..128 {
        assert_eq!(old_get(&mut old, &[31; 32]), new.get(&[31; 32]));
        assert_state(&old, &new);
    }
    assert_eq!(new.consecutive_hits, MAX_ENTRIES * 2);
    for key in [[31; 32], [254; 32]] {
        for image in [&values[4], &values[1]] {
            old_insert(&mut old, key, image.clone());
            new.insert_borrowed(key, image);
            assert_state(&old, &new);
        }
    }
}

#[test]
#[ignore = "paired clone-cost benchmark, run selfdev with --ignored --nocapture"]
fn benchmark_borrowed_candidates() {
    let image = tests::image(300_000);
    for round in 0..6 {
        // Alternate execution order to reduce systematic warmup/order bias.
        for new_first in [round % 2 == 0, round % 2 != 0] {
            let mut cache = ThumbnailCache::new();
            for n in 0..MAX_ENTRIES {
                cache.insert_borrowed([n as u8; 32], &None);
            }
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                // A large frequency-rejected candidate exercises the full policy.
                let key = [255; 32];
                cache.frequency = [[15; 128]; 4];
                cache.observations = 0;
                if new_first {
                    cache.insert_borrowed(key, std::hint::black_box(&image));
                } else {
                    old_insert(&mut cache, key, std::hint::black_box(&image).clone());
                }
                assert_eq!(cache.entries.len(), MAX_ENTRIES);
                assert!(!cache.entries.iter().any(|entry| entry.0 == key));
            }
            eprintln!(
                "round={round} borrowed={new_first} rejected=1000 payload_bytes={} elapsed_us={}",
                retained_bytes(&image),
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
#[ignore = "paired unchanged-hit and duplicate benchmark, run selfdev"]
fn benchmark_unchanged_and_duplicate() {
    let image = tests::image(300_000);
    for round in 0..6 {
        for borrowed in [round % 2 == 0, round % 2 != 0] {
            for duplicate in [false, true] {
                let mut cache = ThumbnailCache::new();
                cache.insert_borrowed([1; 32], &image);
                let start = std::time::Instant::now();
                for _ in 0..1000 {
                    if duplicate {
                        if borrowed {
                            cache.insert_borrowed([1; 32], std::hint::black_box(&image));
                        } else {
                            old_insert(&mut cache, [1; 32], std::hint::black_box(&image).clone());
                        }
                    } else {
                        let output = if borrowed {
                            cache.get(&[1; 32])
                        } else {
                            old_get(&mut cache, &[1; 32])
                        };
                        std::hint::black_box(output);
                    }
                }
                eprintln!(
                    "round={round} borrowed={borrowed} duplicate={duplicate} requests=1000 elapsed_us={}",
                    start.elapsed().as_micros()
                );
            }
        }
    }
}
