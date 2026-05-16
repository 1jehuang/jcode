# fix_remaining_errors.ps1
$enc = [System.Text.Encoding]::UTF8

# Fix 1: request_router.rs - Cell -> Atomic, deref fixes
Write-Host "request_router.rs..." -NoNewline
$p = "crates/jcode-unified-scheduler/src/request_router.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace 'std::cell::Cell<usize>', 'std::sync::atomic::AtomicUsize'
$c = $c -replace 'std::cell::Cell::new\(0\)', 'std::sync::atomic::AtomicUsize::new(0)'
$c = $c -replace 'self.counter.get\(\) % candidates.len\(\);\s+self.counter.set\(self.counter.get\(\) \+ 1\)', 'self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % candidates.len()'
$c = $c -replace 'std::cell::Cell<u64>', 'std::sync::atomic::AtomicU64'
$c = $c -replace 'std::cell::Cell::new\(42\)', 'std::sync::atomic::AtomicU64::new(42)'
$c = $c -replace '\*lat\)', 'lat)'
$c = $c -replace '\(\*idx\)', '(idx)'
$c = $c -replace 'prev = Some\(node\);', 'prev = Some(nodes_map.get(nid).unwrap());'
[System.IO.File]::WriteAllText($p, $c, $enc)
Write-Host " OK"

# Fix 2: goap_planner.rs - multiple issues
Write-Host "goap_planner.rs..." -NoNewline
$p = "crates/jcode-unified-scheduler/src/goap_planner.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace '\.filter\(\|a\| a\.effects\.iter\(\)\.any\(\|e\| e\.key == key\)\)\s+\.map\(\|a\| a\.cost\)\s+\.min\(\)', '.filter(|a| a.effects.iter().any(|e| e.key == key)).map(|a| a.cost).reduce(f64::min)'
$c = $c -replace '\.map\(\|&\(idx, step_num\)\|', '.map(|(idx, step_num)|'
$c = $c -replace 'if let Some\(ref meta\) = task.metadata', 'let meta = &task.metadata; if meta.is_object()'
$c = $c -replace 'lang\.as_str\(\)\.unwrap_or\("unknown"\)', 'lang.as_str().unwrap_or("unknown")'
$c = $c -replace 'if has_tests\.as_bool\(\)\.unwrap_or\(false\)', 'let has_tests_bool = has_tests; if has_tests_bool.as_bool().unwrap_or(false)'
$c = $c -replace 'state\.set\("dependencies_installed"\.into\(\)', 'state.set("dependencies_installed"'
$c = $c -replace 'best_partial = Some\(current\);', 'best_partial = Some(current.clone());'
$c = $c -replace 'let prev_total_ns =', 'let _prev_total_ns ='
[System.IO.File]::WriteAllText($p, $c, $enc)
Write-Host " OK"

# Fix 3: lib.rs - borrow/lifetime/Debug
Write-Host "lib.rs..." -NoNewline
$p = "crates/jcode-unified-scheduler/src/lib.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)

# plan borrow: need to extract steps before storing plan
$c = $c -replace 'Ok\(plan\) => \{\s+info!\(', 'Ok(ref plan) => { info!('
$c = $c -replace 'task\.plan = Some\(plan\);', 'task.plan = Some(plan.clone());'

# task.id used after queue.push(task) - extract id first
$c = $c -replace 'queue\.push\(task\)\?;\s+}', 'let task_id_for_return = task.id; queue.push(task)?; }'
$c = $c -replace 'Ok\(task\.id\)', 'Ok(task_id_for_return)'

# node.add_request() can't borrow Arc as mutable
$c = $c -replace '(\s+)node\.add_request\(\);', '${1}Arc::make_mut(&mut node).add_request();'

# Debug derives for NodeManager, UnifiedQueue, LayerAllocator, RequestRouter
# These need to be added in their respective files, not here
# Remove the unnecessary `task` param
$c = $c -replace 'task: &ScheduledTask,', '_task: &ScheduledTask,'

[System.IO.File]::WriteAllText($p, $c, $enc)
Write-Host " OK"

# Fix 4: resource_node.rs - chrono Duration
Write-Host "resource_node.rs..." -NoNewline
$p = "crates/jcode-unified-scheduler/src/resource_node.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace 'unwrap_or\(Duration::ZERO\)', 'unwrap_or(std::time::Duration::ZERO)'
[System.IO.File]::WriteAllText($p, $c, $enc)
Write-Host " OK"

Write-Host "ALL DONE" -ForegroundColor Green
