use super::*;

#[path = "final_text_tests.rs"]
mod final_text_tests;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

// Counting is per-thread and disabled during CPU measurements and other tests.
struct ProbeAllocator;
thread_local! {
    static COUNT: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}
fn record(bytes: usize) {
    if COUNT.try_with(Cell::get).unwrap_or(false) {
        CALLS.with(|n| n.set(n.get() + 1));
        BYTES.with(|n| n.set(n.get() + bytes));
    }
}
unsafe impl GlobalAlloc for ProbeAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: ProbeAllocator = ProbeAllocator;

type Output = (
    Vec<RenderedMessage>,
    Vec<RenderedImage>,
    Option<RenderedCompactedHistoryInfo>,
);
fn original(session: &Session) -> Output {
    image_context_original::render_messages_and_images_with_compacted_history(session, 64)
}
fn optimized(session: &Session) -> Output {
    render_messages_and_images_with_compacted_history(session, 64)
}
fn text(s: &str) -> ContentBlock {
    ContentBlock::Text {
        text: s.into(),
        cache_control: None,
    }
}
fn image() -> ContentBlock {
    ContentBlock::Image {
        media_type: "image/png".into(),
        data: "YWJj".into(),
    }
}
fn call(id: &str, size: usize) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: "read".into(),
        input: serde_json::json!({"file_path": " /tmp/test.png ", "intent": "inspect image", "payload": "x".repeat(size)}),
        thought_signature: Some("signature".into()),
    }
}
fn result(id: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.into(),
        content: "result".into(),
        is_error: None,
    }
}
fn native() -> ContentBlock {
    ContentBlock::ProviderNative {
        provider: "openai".into(),
        item: serde_json::json!({"type":"web_search_call", "id":"native", "status":"completed", "action":{"type":"search", "query":"test"}}),
    }
}
fn fixture(size: usize, images: bool) -> Session {
    let mut s = Session::create_with_id("render-image-context-probe".into(), None, None);
    s.add_message(Role::Assistant, vec![call("a", size)]);
    let mut blocks = vec![result("a")];
    if images {
        blocks.push(image());
    }
    s.add_message(Role::User, blocks);
    s
}
fn parity(session: &Session) {
    // RenderedMessage intentionally has no PartialEq. Serialization compares all fields.
    assert_eq!(
        serde_json::to_value(original(session)).unwrap(),
        serde_json::to_value(optimized(session)).unwrap()
    );
}
#[test]
fn image_context_frozen_renderer_parity() {
    for size in [0, 32, 256 * 1024] {
        for images in [false, true] {
            parity(&fixture(size, images));
        }
    }
    for role in [Role::User, Role::Assistant] {
        let mut s = Session::create_with_id("image-edges".into(), None, None);
        s.add_message(Role::Assistant, vec![call("a", 0), call("b", 0)]);
        s.add_message(
            role,
            vec![
                image(),
                text("prompt"),
                result("a"),
                image(),
                image(),
                text("[Attached image associated with the preceding tool result: explicit.png]"),
                text("interleaved"),
                native(),
                image(),
                call("a", 8192),
                image(),
                result("a"),
                image(),
                result("missing"),
                image(),
                result("b"),
                image(),
            ],
        );
        parity(&s);
        let (_, images, _) = optimized(&s);
        assert_eq!(images[2].label.as_deref(), Some("explicit.png"));
        assert_eq!(
            images[4].anchor,
            Some(RenderedImageAnchor::ToolCall { id: "a".into() })
        );
        assert_eq!(
            images[6].source,
            RenderedImageSource::ToolResult {
                tool_name: "tool".into()
            }
        );
    }
    for input in [
        serde_json::json!({}),
        serde_json::json!({"file_path":"   "}),
        serde_json::json!({"file_path":42}),
    ] {
        let mut s = Session::create_with_id("fallback-label".into(), None, None);
        s.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "fallback".into(),
                name: "read".into(),
                input,
                thought_signature: None,
            }],
        );
        s.add_message(Role::User, vec![result("fallback"), image()]);
        parity(&s);
        assert!(optimized(&s).1[0].label.is_none());
    }
    for blocks in [
        vec![image()],
        vec![image(), text("prompt")],
        vec![native(), image()],
    ] {
        let mut s = Session::create_with_id("unowned-image".into(), None, None);
        s.add_message(Role::User, blocks);
        parity(&s);
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) },
        0
    );
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
fn allocations(f: fn(&Session) -> Output, s: &Session) -> (usize, usize) {
    CALLS.with(|n| n.set(0));
    BYTES.with(|n| n.set(0));
    COUNT.with(|n| n.set(true));
    drop(black_box(f(black_box(s))));
    COUNT.with(|n| n.set(false));
    (CALLS.with(Cell::get), BYTES.with(Cell::get))
}

// Run alone: justrust test -p jcode-base --lib image_context_cpu_allocation_probe -- --ignored --nocapture --test-threads=1
// CPU uses the calling thread's actual clock, not elapsed time. Counting is separate.
#[test]
#[ignore = "isolated alternating CPU/allocation measurement"]
#[cfg(target_os = "linux")]
fn image_context_cpu_allocation_probe() {
    let empty = Session::create_with_id("empty-render-probe".into(), None, None);
    for (name, s) in [
        ("noimages", empty),
        ("tiny", fixture(8, false)),
        ("largeinput", fixture(256 * 1024, false)),
        ("images", fixture(256 * 1024, true)),
    ] {
        parity(&s);
        for _ in 0..64 {
            drop(original(&s));
            drop(optimized(&s));
        }
        let old_alloc = allocations(original, &s);
        let new_alloc = allocations(optimized, &s);
        assert!(new_alloc.0 <= old_alloc.0 && new_alloc.1 <= old_alloc.1);
        let mut old = Vec::new();
        let mut new = Vec::new();
        let iterations = if name == "largeinput" || name == "images" {
            500
        } else {
            10_000
        };
        let measure = |f: fn(&Session) -> Output| {
            let start = thread_cpu_ns();
            for _ in 0..iterations {
                drop(black_box(f(black_box(&s))));
            }
            thread_cpu_ns() - start
        };
        for round in 0..12 {
            if round % 2 == 0 {
                old.push(measure(original));
                new.push(measure(optimized));
            } else {
                new.push(measure(optimized));
                old.push(measure(original));
            }
        }
        old.sort_unstable();
        new.sort_unstable();
        let ratio = new[6] as f64 / old[6] as f64;
        println!(
            "{name}: CPU new/old={ratio:.4}, old={}ns new={}ns allocations {old_alloc:?} -> {new_alloc:?}",
            old[6], new[6]
        );
        assert!(ratio <= 1.05, "{name}: CPU regression {ratio}");
    }
}
