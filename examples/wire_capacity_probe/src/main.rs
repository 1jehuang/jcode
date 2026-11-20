//! Linux-only controlled serialization experiment, not a production implementation.
//! Run optimized, pinned to one CPU when possible. CPU and allocation modes are separate:
//! `wire_capacity_probe cpu [rounds]` and `wire_capacity_probe alloc`.
use serde_json::{Value, json};
#[cfg(feature = "allocation-meter")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

#[cfg(feature = "allocation-meter")]
struct Meter;
static ENABLED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "allocation-meter")]
fn allocated(size: usize) {
    CALLS.fetch_add(1, Relaxed);
    REQUESTED.fetch_add(size, Relaxed);
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}
#[cfg(feature = "allocation-meter")]
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && ENABLED.load(Relaxed) {
            allocated(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ENABLED.load(Relaxed) {
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, size) };
        if !next.is_null() && ENABLED.load(Relaxed) {
            // realloc's old and new allocation need not coexist. Report logical
            // live bytes, not allocator RSS or an assumed copying peak.
            LIVE.fetch_sub(layout.size(), Relaxed);
            allocated(size);
        }
        next
    }
}
#[cfg(feature = "allocation-meter")]
#[global_allocator]
static ALLOC: Meter = Meter;

struct Growth(Vec<u8>);
impl Write for Growth {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let needed = self.0.len() + bytes.len();
        if needed > self.0.capacity() {
            let target = needed
                .max(self.0.capacity() + self.0.capacity() / 2)
                .max(128);
            self.0.reserve_exact(target - self.0.len());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
enum Method {
    Baseline,
    Shrink,
    Growth,
    Conditional,
}
fn shrink_large(bytes: &mut Vec<u8>) {
    if bytes.len() >= 64 * 1024 && bytes.capacity() - bytes.len() >= bytes.capacity() / 4 {
        bytes.shrink_to_fit();
    }
}
fn encode(value: &Value, method: Method) -> Vec<u8> {
    match method {
        Method::Baseline => serde_json::to_vec(value).unwrap(),
        Method::Shrink => {
            let mut bytes = serde_json::to_vec(value).unwrap();
            bytes.shrink_to_fit();
            bytes
        }
        Method::Conditional => {
            let mut bytes = serde_json::to_vec(value).unwrap();
            shrink_large(&mut bytes);
            bytes
        }
        Method::Growth => {
            let mut writer = Growth(Vec::with_capacity(128));
            serde_json::to_writer(&mut writer, value).unwrap();
            writer.0
        }
    }
}
fn fixtures() -> Vec<(&'static str, Value)> {
    let mut state = 0x123456789abcdefu64;
    let mut noisy = String::with_capacity(1_051_337);
    for _ in 0..1_051_337 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        noisy.push((32 + (state % 95) as u8) as char);
    }
    vec![
        (
            "tiny",
            json!({"model":"test", "messages":[{"role":"user","content":"hello"}]}),
        ),
        (
            "escaped",
            json!({"text":"\n\t\r\"\\\u{0001}é🦀".repeat(18_001)}),
        ),
        (
            "nested",
            json!({"messages":(0..700).map(|i| json!({"role":"user", "content":[{"type":"text","text":format!("message {i}: {}", "context ".repeat(23))}]})).collect::<Vec<_>>()}),
        ),
        ("noisy", json!({"text":noisy})),
        (
            "large",
            json!({"messages":(0..101).map(|i| json!({"role":"user","content":format!("turn {i}: {}", "The quick brown fox jumps over the lazy dog. ".repeat(241))})).collect::<Vec<_>>()}),
        ),
    ]
}
fn conditional_fixtures() -> Vec<(&'static str, Value)> {
    let mut values = fixtures();
    for (name, len) in [
        ("below-threshold", 65535),
        ("at-threshold", 65536),
        ("above-threshold", 65537),
    ] {
        values.push((name, json!({"text":"a".repeat(len - 11)})));
    }
    values.push(("many-small", json!({"messages":(0..20_000).map(|i| json!({"role":"user","content":format!("turn {i}")})).collect::<Vec<_>>()})));
    values.push(("native-8x1MiB", json!({"model":"native-test","stream":true,"messages":(0..8).map(|_| json!({"role":"user","content":[{"type":"text","text":"x".repeat(1024*1024)}]})).collect::<Vec<_>>()})));
    values
}
fn thread_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
        0
    );
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
fn sample(value: &Value, method: Method, iterations: usize) -> u64 {
    let start = thread_ns();
    for _ in 0..iterations {
        black_box(encode(black_box(value), method));
    }
    thread_ns() - start
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "cpu".into());
    let conditional = mode.ends_with("-conditional");
    let allocation = mode.starts_with("alloc");
    let methods: &[Method] = if conditional {
        &[Method::Conditional]
    } else {
        &[Method::Shrink, Method::Growth]
    };
    assert_eq!(
        allocation,
        cfg!(feature = "allocation-meter"),
        "build CPU mode without allocation-meter, allocation mode with it"
    );
    let rounds: usize = std::env::args()
        .nth(2)
        .map(|x| x.parse().unwrap())
        .unwrap_or(15);
    for (name, value) in if conditional {
        conditional_fixtures()
    } else {
        fixtures()
    } {
        let expected = encode(&value, Method::Baseline);
        for &method in methods {
            assert_eq!(encode(&value, method), expected);
        }
        if allocation {
            for method in std::iter::once(Method::Baseline).chain(methods.iter().copied()) {
                CALLS.store(0, Relaxed);
                REQUESTED.store(0, Relaxed);
                LIVE.store(0, Relaxed);
                PEAK.store(0, Relaxed);
                ENABLED.store(true, Relaxed);
                let bytes = encode(&value, method);
                ENABLED.store(false, Relaxed);
                println!(
                    "alloc {name} {method:?} len={} cap={} calls={} requested={} retained={} peak={}",
                    bytes.len(),
                    bytes.capacity(),
                    CALLS.load(Relaxed),
                    REQUESTED.load(Relaxed),
                    LIVE.load(Relaxed),
                    PEAK.load(Relaxed)
                );
                drop(bytes);
            }
        } else {
            let iterations = (16_000_000 / expected.len()).clamp(8, 100_000);
            for &method in methods {
                for _ in 0..3 {
                    sample(&value, method, iterations);
                    sample(&value, Method::Baseline, iterations);
                }
                let mut ratios = Vec::new();
                for round in 0..rounds {
                    let (baseline, candidate) = if round % 2 == 0 {
                        let b = sample(&value, Method::Baseline, iterations);
                        (b, sample(&value, method, iterations))
                    } else {
                        let c = sample(&value, method, iterations);
                        (sample(&value, Method::Baseline, iterations), c)
                    };
                    let ratio = candidate as f64 / baseline as f64;
                    ratios.push(ratio);
                    println!(
                        "cpu {name} {method:?} round={round} n={iterations} baseline_ns={baseline} candidate_ns={candidate} ratio={ratio:.5}"
                    );
                }
                ratios.sort_by(f64::total_cmp);
                println!(
                    "summary {name} {method:?} median_ratio={:.5} min={:.5} max={:.5}",
                    ratios[ratios.len() / 2],
                    ratios[0],
                    ratios[ratios.len() - 1]
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::SerializeSeq;

    #[test]
    fn conditional_preserves_bytes_and_only_shrinks_large_slack() {
        for (name, value) in conditional_fixtures() {
            assert_eq!(
                encode(&value, Method::Conditional),
                encode(&value, Method::Baseline),
                "{name}"
            );
        }
        for (len, capacity, shrink) in [
            (65535, 131072, false),
            (65536, 131072, true),
            (65537, 131072, true),
            (65536, 65536, false),
            (65536, 87381, true),
            (65536, 87380, false),
        ] {
            let mut bytes = Vec::with_capacity(capacity);
            bytes.resize(len, 42);
            let old_capacity = bytes.capacity();
            shrink_large(&mut bytes);
            assert_eq!(bytes.capacity(), if shrink { len } else { old_capacity });
            assert!(bytes.iter().all(|b| *b == 42));
        }
    }

    #[test]
    fn candidates_preserve_exact_fixture_bytes() {
        for (name, value) in fixtures() {
            let baseline = encode(&value, Method::Baseline);
            for method in [Method::Shrink, Method::Growth] {
                assert_eq!(encode(&value, method), baseline, "{name}: {method:?}");
            }
        }
    }

    #[test]
    fn growth_preserves_partial_serialization_errors_and_single_pass() {
        struct Failing<'a>(&'a AtomicUsize);
        impl serde::Serialize for Failing<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.fetch_add(1, Relaxed);
                let mut seq = serializer.serialize_seq(Some(2))?;
                seq.serialize_element("written before failure")?;
                Err(serde::ser::Error::custom(
                    "intentional serialization failure",
                ))
            }
        }
        let calls = AtomicUsize::new(0);
        let baseline = serde_json::to_vec(&Failing(&calls)).unwrap_err();
        assert_eq!(calls.swap(0, Relaxed), 1);
        let mut writer = Growth(Vec::with_capacity(128));
        let candidate = serde_json::to_writer(&mut writer, &Failing(&calls)).unwrap_err();
        assert_eq!(calls.load(Relaxed), 1);
        assert!(!writer.0.is_empty());
        assert_eq!(candidate.classify(), baseline.classify());
        assert_eq!(candidate.to_string(), baseline.to_string());
        assert_eq!(candidate.line(), baseline.line());
        assert_eq!(candidate.column(), baseline.column());
    }
}
