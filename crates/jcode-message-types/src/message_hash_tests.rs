use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

thread_local! {
    static ALLOCATIONS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some((calls, bytes)) = count.get() {
                count.set(Some((calls + 1, bytes + layout.size())));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| {
            if let Some((calls, bytes)) = count.get() {
                count.set(Some((calls + 1, bytes + size)));
            }
        });
        unsafe { System.realloc(ptr, old, size) }
    }
}

fn legacy(message: &Message) -> u64 {
    let metadata = match serde_json::to_vec(&message.metadata_only()) {
        Ok(metadata) => metadata,
        Err(_) => return stable_hash_bytes(format!("{:?}", message).as_bytes()),
    };
    message
        .content
        .cached_message_hash(&metadata, || match serde_json::to_vec(message) {
            Ok(bytes) => {
                message
                    .content
                    .record_serialized_json_bytes(bytes.len().saturating_sub(metadata.len()) + 2);
                stable_hash_bytes(&bytes)
            }
            Err(_) => stable_hash_bytes(format!("{:?}", message).as_bytes()),
        })
}

fn cold(message: &Message) -> Message {
    Message {
        role: message.role.clone(),
        content: message.content.iter().cloned().collect::<Vec<_>>().into(),
        timestamp: message.timestamp,
        tool_duration_ms: message.tool_duration_ms,
    }
}

fn shapes() -> Vec<Message> {
    let blocks = serde_json::json!([
        {"type":"text", "text":"\u{0000}\u{0001}\t\r\n\"\\é中🦀", "cache_control":{"type":"ephemeral","ttl":"5m"}},
        {"type":"reasoning","text":"reason"},
        {"type":"reasoning_trace","text":"trace"},
        {"type":"anthropic_thinking","thinking":"think","signature":"sig"},
        {"type":"open_a_i_reasoning","id":"id","summary":["a","b"],"encrypted_content":"enc","status":"done"},
        {"type":"tool_use","id":"id","name":"tool","input":{"nested":[null,true,false,1,-2,1.25,{"é":"\\"}]},"thought_signature":"sig"},
        {"type":"tool_result","tool_use_id":"id","content":"result","is_error":false},
        {"type":"image","media_type":"image/png","data":"aGVsbG8="},
        {"type":"open_a_i_compaction","encrypted_content":"enc"},
        {"type":"tool_reference","tool_use_id":"id","tool_name":"tool"},
        {"type":"provider_native","provider":"any","item":{"n":18446744073709551615u64}}
    ]);
    let all: Vec<ContentBlock> = serde_json::from_value(blocks).unwrap();
    let mut result = vec![Message {
        role: Role::User,
        content: Vec::new().into(),
        timestamp: None,
        tool_duration_ms: None,
    }];
    for block in &all {
        result.push(Message {
            role: Role::Assistant,
            content: vec![block.clone()].into(),
            timestamp: None,
            tool_duration_ms: Some(u64::MAX),
        });
    }
    result.push(Message {
        role: Role::User,
        content: all.into(),
        timestamp: Some("2026-11-18T06:00:00.123456789Z".parse().unwrap()),
        tool_duration_ms: Some(0),
    });
    result
}

#[test]
fn legacy_parity_and_cache_isolation() {
    for message in shapes() {
        let expected = legacy(&cold(&message));
        assert_eq!(stable_message_hash(&message), expected);
        assert_eq!(
            message.serialized_json_bytes(),
            serde_json::to_vec(&message).unwrap().len()
        );
        assert_eq!(stable_message_hash(&message), expected);
        let mut changed = message.clone();
        changed.role = if changed.role == Role::User {
            Role::Assistant
        } else {
            Role::User
        };
        changed.timestamp = None;
        changed.tool_duration_ms = Some(42);
        assert_eq!(stable_message_hash(&changed), legacy(&cold(&changed)));
        assert_eq!(stable_message_hash(&message), expected);
        changed.content.push(ContentBlock::Text {
            text: "mutated".into(),
            cache_control: None,
        });
        assert_eq!(stable_message_hash(&changed), legacy(&cold(&changed)));
        assert_eq!(
            changed.serialized_json_bytes(),
            serde_json::to_vec(&changed).unwrap().len()
        );
        assert_eq!(stable_message_hash(&message), expected);
    }
}

#[test]
fn controls_unicode_and_individual_metadata_changes() {
    let text: String = (0..=127)
        .map(char::from)
        .chain("é中🦀\u{2028}\u{2029}".chars())
        .collect();
    let original = Message::user(&text);
    let expected = legacy(&cold(&original));
    assert_eq!(stable_message_hash(&original), expected);
    for index in 0..3 {
        let mut changed = original.clone();
        match index {
            0 => changed.role = Role::Assistant,
            1 => changed.timestamp = None,
            _ => changed.tool_duration_ms = Some(u64::MAX),
        }
        let changed_expected = legacy(&cold(&changed));
        assert_ne!(changed_expected, expected);
        assert_eq!(stable_message_hash(&changed), changed_expected);
        assert_eq!(
            changed.content.cached_message_hash(
                &serde_json::to_vec(&changed.metadata_only()).unwrap(),
                || panic!("hash cache not populated"),
            ),
            changed_expected
        );
        assert_eq!(stable_message_hash(&original), expected);
    }
}

// No current Message field has a fallible Serialize implementation. Verify that
// serde errors remain errors rather than accepting a partial hash.
#[test]
fn writer_preserves_serialization_errors() {
    struct Fails;
    impl serde::Serialize for Fails {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("deliberate"))
        }
    }
    let mut writer = MessageHashWriter {
        hash: STABLE_HASH_SEED,
        len: 0,
    };
    assert!(serde_json::to_writer(&mut writer, &Fails).is_err());
    assert!(serde_json::to_vec(&Fails).is_err());
}

#[cfg(target_os = "linux")]
fn cpu_ns() -> u128 {
    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanos: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec {
        seconds: 0,
        nanos: 0,
    };
    assert_eq!(unsafe { clock_gettime(3, &mut time) }, 0); // CLOCK_THREAD_CPUTIME_ID
    time.seconds as u128 * 1_000_000_000 + time.nanos as u128
}

/// Hosted measurements, 2026-11-18, optimized pinned jr-ac20de5e7921,
/// two runs, 20 alternating rounds each. Thread CPU ns/message (legacy -> new):
/// small 730/724 -> 509/504, mixed 3138/3142 -> 1844/1826,
/// ASCII 64KiB 74173/74287 -> 73265/73379,
/// escape-heavy 241147/236817 -> 88274/88721,
/// ASCII 1MiB 1426843/1436758 -> 1165431/1173024.
/// Alloc calls / cumulative allocated bytes (identical in both runs):
/// small 3/329 -> 2/201, mixed 6/2142 -> 2/222,
/// ASCII 64KiB 5/197084 -> 2/201, escape-heavy 13/262217 -> 2/201,
/// ASCII 1MiB 5/3146204 -> 2/201. Wall latency tracked CPU closely,
/// with no regression in either run. Measurements are not runtime assertions.
///
/// Reproduce with:
/// justrust test -p jcode-message-types --release benchmark_message_hash -- --ignored --nocapture --test-threads=1
/// Each timed batch gets independently allocated cold content, prepared outside
/// timing. Alternating order controls warmup/order bias. Allocations are sampled
/// separately so instrumentation does not distort latency. Reports cumulative
/// allocated bytes, including reallocations, not retained or peak bytes.
#[test]
#[ignore]
#[cfg(target_os = "linux")]
fn benchmark_message_hash() {
    let mut cases = vec![
        ("small", Message::user("hello")),
        ("mixed", shapes().pop().unwrap()),
    ];
    for (name, text) in [
        ("ascii-64k", "a".repeat(65536)),
        ("escaped-64k", "\n\t\"\\中🦀".repeat(6554)),
        ("ascii-1m", "x".repeat(1048576)),
    ] {
        cases.push((name, Message::user(&text)));
    }
    for (name, template) in cases {
        let count = if name == "ascii-1m" { 32 } else { 256 };
        let mut wall = [0u128; 2];
        let mut cpu = [0u128; 2];
        let functions: [fn(&Message) -> u64; 2] = [legacy, stable_message_hash];
        for round in 0..20 {
            for index in [round % 2, 1 - round % 2] {
                let messages: Vec<_> = (0..count).map(|_| cold(&template)).collect();
                let start_cpu = cpu_ns();
                let start = Instant::now();
                for message in &messages {
                    black_box(functions[index](black_box(message)));
                }
                wall[index] += start.elapsed().as_nanos();
                cpu[index] += cpu_ns() - start_cpu;
            }
        }
        let mut allocations = [(0, 0); 2];
        for index in 0..2 {
            let message = cold(&template);
            ALLOCATIONS.with(|counter| counter.set(Some((0, 0))));
            black_box(functions[index](&message));
            allocations[index] = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
        }
        let n = (count * 20) as u128;
        println!(
            "{name}: legacy wall={}ns cpu={}ns alloc={:?}; streaming wall={}ns cpu={}ns alloc={:?}; CPU ratio={:.4}",
            wall[0] / n,
            cpu[0] / n,
            allocations[0],
            wall[1] / n,
            cpu[1] / n,
            allocations[1],
            cpu[1] as f64 / cpu[0] as f64
        );
    }
}
