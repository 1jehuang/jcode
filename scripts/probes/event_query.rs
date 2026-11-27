// Included after the production event definitions and formatting functions.
fn render<'a>(events: impl Iterator<Item = &'a SwarmEvent> + Clone, new: bool) -> String {
    if new {
        pretty_events(events)
    } else {
        serde_json::to_string_pretty(&events.map(event_payload).collect::<Vec<_>>()).unwrap()
    }
}
fn ns() -> u64 {
    unsafe {
        let mut t: libc::timespec = std::mem::zeroed();
        assert_eq!(
            libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t),
            0
        );
        (t.tv_sec * 1000000000 + t.tv_nsec) as u64
    }
}
#[cfg(feature = "count")]
mod alloc {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicUsize, Ordering::Relaxed},
    };
    pub static N: AtomicUsize = AtomicUsize::new(0);
    pub static B: AtomicUsize = AtomicUsize::new(0);
    struct A;
    unsafe impl GlobalAlloc for A {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            N.fetch_add(1, Relaxed);
            B.fetch_add(l.size(), Relaxed);
            System.alloc(l)
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            N.fetch_add(1, Relaxed);
            B.fetch_add(n, Relaxed);
            System.realloc(p, l, n)
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            System.dealloc(p, l)
        }
    }
    #[global_allocator]
    static A_: A = A;
    pub fn snap() -> (usize, usize) {
        (N.load(Relaxed), B.load(Relaxed))
    }
}
#[test]
fn trial() {
    for n in [0, 1, 50, 5000] {
        let h: VecDeque<_> = (0..n)
            .map(|id| SwarmEvent {
                id,
                session_id: format!("s-{id}"),
                session_name: None,
                swarm_id: Some("swarm".into()),
                event: SwarmEventType::Notification {
                    notification_type: "info".into(),
                    message: "é\n\"\\".repeat(12),
                },
                timestamp: Instant::now(),
                absolute_time: SystemTime::UNIX_EPOCH,
            })
            .collect();
        let lock = std::sync::RwLock::new(h);
        for since in [false, true] {
            let run = |new| {
                let h = lock.read().unwrap();
                if since {
                    render(h.iter().filter(|e| e.id > n / 2), new)
                } else {
                    render(h.iter().rev().take(n as usize), new)
                }
            };
            assert_eq!(run(false), run(true));
            #[cfg(feature = "count")]
            {
                for k in [false, true] {
                    let a = alloc::snap();
                    black_box(run(k));
                    let b = alloc::snap();
                    println!(
                        "alloc {n} since={since} new={k}: calls {} bytes {}",
                        b.0 - a.0,
                        b.1 - a.1
                    )
                }
            }
            #[cfg(not(feature = "count"))]
            {
                let reps = (20000 / (n + 1)).max(2);
                let mut wall = [vec![], vec![]];
                let mut v = [vec![], vec![]];
                for round in 0..10 {
                    for j in 0..2 {
                        let k = (j + round) % 2;
                        let start = Instant::now();
                        let t = ns();
                        for _ in 0..reps {
                            black_box(run(k != 0));
                        }
                        v[k].push((ns() - t) / reps);
                        wall[k].push(start.elapsed().as_nanos() as u64 / reps);
                    }
                }
                for t in &mut wall {
                    t.sort()
                }
                println!("wall ns {n} since={since}: {} {}", wall[0][5], wall[1][5]);
                for t in &mut v {
                    t.sort()
                }
                println!("ns {n} since={since} old {} stream {}", v[0][5], v[1][5]);
            }
        }
    }
}
