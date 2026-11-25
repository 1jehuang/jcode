#!/usr/bin/env bash
# Compile the production Debug derives and estimator in isolation, without a server build.
set -euo pipefail
cd "$(dirname "$0")/.."
d=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/event-bytes.XXXXXX")
trap 'rm -rf "$d"' EXIT
src=crates/jcode-app-core/src/server
{
  echo 'use std::{hint::black_box, time::{Instant,SystemTime}, collections::VecDeque};'
  for item in 'enum SwarmEventType' 'struct SwarmEvent'; do
    echo '#[derive(Clone, Debug)]'
    sed -n "/^pub $item {/,/^}/p" "$src/state.rs" | grep -v '#\[serde'
  done
  sed -n '/^fn estimate_swarm_event_bytes(/,/^}/p' "$src/debug_server_state.rs"
  cat <<'RS'
#[cfg(measure_alloc)]
mod alloc {
    use std::{alloc::{GlobalAlloc, System, Layout}, sync::atomic::{AtomicUsize, Ordering::Relaxed}};
    pub static CALLS: AtomicUsize = AtomicUsize::new(0);
    pub struct Counter;
    unsafe impl GlobalAlloc for Counter {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 { CALLS.fetch_add(1,Relaxed); System.alloc(l) }
        unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8 { CALLS.fetch_add(1,Relaxed); System.realloc(p,l,n) }
        unsafe fn dealloc(&self,p:*mut u8,l:Layout) { System.dealloc(p,l) }
    }
    #[global_allocator] static A: Counter = Counter;
}
fn old(e:&SwarmEvent)->usize { format!("{e:?}").len() }
fn main() {
    for (label,n,msg,iterations) in [("empty",0,"".into(),1_000_000),("common",64,"peer joined".into(),400),
        ("escaped",64,"\0\n\"\\\t✨é🦀".repeat(8),200),("large",4,"long detail\n".repeat(8192),12)] {
        let events:VecDeque<_>=(0..n).map(|id| SwarmEvent { id, session_id:format!("s-{id}"),session_name:None,
            swarm_id:Some("swarm".into()),event:SwarmEventType::Notification {notification_type:"info".into(),message:msg.clone()},
            timestamp:Instant::now(),absolute_time:SystemTime::now() }).collect();
        for e in &events { assert_eq!(old(e),estimate_swarm_event_bytes(e)); }
        #[cfg(measure_alloc)] {
            let _=iterations;
            let capacity:usize=events.iter().map(|e|format!("{e:?}").capacity()).sum();
            for f in [old,estimate_swarm_event_bytes] { use std::sync::atomic::Ordering::Relaxed;
                let before=alloc::CALLS.load(Relaxed); black_box(events.iter().map(f).sum::<usize>());
                let calls=alloc::CALLS.load(Relaxed)-before;
                println!("{label}: alloc/realloc {calls}, legacy summed final capacities {capacity}");
            }
        }
        #[cfg(not(measure_alloc))] {
            let run=|f:fn(&SwarmEvent)->usize| { let start=Instant::now(); for _ in 0..iterations {
                black_box(black_box(&events).iter().map(f).sum::<usize>()); } start.elapsed().as_nanos() as f64/iterations as f64 };
            let mut times=[vec![],vec![]];
            for round in 0..12 { for k in 0..2 { let i=(round+k)%2; times[i].push(run([old,estimate_swarm_event_bytes][i])); } }
            for t in &mut times { t.sort_by(f64::total_cmp); }
            println!("{label}: median ns/history old {:.1} candidate {:.1}",(times[0][5]+times[0][6])/2.,(times[1][5]+times[1][6])/2.);
        }
    }
}
RS
} > "$d/probe.rs"
for cfg in '' '--cfg measure_alloc'; do
  rustc --edition=2021 -O $cfg "$d/probe.rs" -o "$d/probe"
  "$d/probe"
done
