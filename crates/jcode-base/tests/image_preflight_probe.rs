//! Isolated allocator: never install this in the shared base unit-test binary.
//! Run optimized on Linux: justrust test -p jcode-base --no-default-features
//! --release --test image_preflight_probe isolated_probe -- --ignored --nocapture
#![cfg(target_os = "linux")]
use base64::Engine as _;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

pub use jcode_base::image_normalize;
#[path = "../src/provider/image_clamp.rs"]
#[allow(dead_code)]
mod image_clamp;

thread_local! {
    static COUNT: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn record(bytes: usize) {
    let _ = COUNT.try_with(|count| {
        if let Some((calls, total)) = count.get() {
            count.set(Some((calls + 1, total + bytes as u64)));
        }
    });
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
fn cpu_ns() -> u64 {
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
fn fixture(width: u32, height: u32, noise: bool) -> String {
    let mut state = 42u32;
    let img = image::RgbImage::from_fn(width, height, |_, _| {
        let mut pixel = [0; 3];
        for byte in &mut pixel {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = if noise { (state >> 24) as u8 } else { 42 };
        }
        image::Rgb(pixel)
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
}
#[test]
#[ignore = "explicit optimized isolated CPU/allocation measurement"]
fn isolated_probe() {
    assert!(
        !cfg!(debug_assertions),
        "use --release for optimized measurements"
    );
    let ordinary = fixture(100, 100, false);
    let large = fixture(1400, 1400, true);
    for (name, data, iterations, expected) in [
        ("no-image", None, 100_000, false),
        ("ordinary", Some(ordinary.as_str()), 100_000, false),
        ("large", Some(large.as_str()), 40, false),
        ("malformed", Some("@@not base64@@"), 100_000, true),
    ] {
        let check = |baseline| image_clamp::preflight_probe("image/png", black_box(data), baseline);
        for baseline in [true, false] {
            assert_eq!(check(baseline), expected, "{name}: frozen oracle");
            COUNT.with(|c| c.set(Some((0, 0))));
            for _ in 0..10 {
                black_box(check(baseline));
            }
            let counts = COUNT.with(|c| c.replace(None).unwrap());
            let expected_calls = if data.is_none() {
                0
            } else if baseline {
                20
            } else {
                10
            };
            assert_eq!(counts.0, expected_calls, "{name}: allocator calls");
            if data.is_none() {
                assert_eq!(counts.1, 0);
            }
            eprintln!(
                "alloc {name} baseline={baseline} calls/10={} requested_bytes/10={}",
                counts.0, counts.1
            );
        }
        // Counter disabled during CPU samples. Reverse order each round to
        // reduce systematic cache/frequency/order bias. Fixtures are outside.
        for round in 0..6 {
            for baseline in if round % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = cpu_ns();
                for _ in 0..iterations {
                    black_box(check(black_box(baseline)));
                }
                let elapsed = cpu_ns() - start;
                eprintln!(
                    "cpu {name} round={round} baseline={baseline} iterations={iterations} ns={elapsed}"
                );
            }
        }
    }
}
