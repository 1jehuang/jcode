# fix_remaining_errors2.ps1
$enc = [System.Text.Encoding]::UTF8

Write-Host "Fixing lib.rs..."
$p = "crates/jcode-unified-scheduler/src/lib.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace 'let nodes = \{\s+let manager = self\.node_manager\.read\(\)\.await;\s+manager\.active_node_list\(\)\s+\};', 'let nodes: Vec<Arc<NodeInfo>> = { let mgr = self.node_manager.read().await; mgr.active_nodes().into_iter().map(|n| Arc::new(n)).collect() };'
$c = $c -replace 'let mgr = self\.node_manager\.read\(\)\.await;\s+mgr\.active_node_list\(\)', 'let mgr = self.node_manager.read().await; mgr.active_nodes()'
$c = $c -replace 'let new_node = None; \/\* removed for lifetime fix \*\/', 'let new_node = None;'
[System.IO.File]::WriteAllText($p, $c, $enc)

Write-Host "Fixing layer_allocator.rs..."
$p = "crates/jcode-unified-scheduler/src/layer_allocator.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace 'solve\(0, vec!\[\], 0, k_target, n, L, suffix_sum, self, &mut memo\)\?;', 'let _solved = solve(0, vec![], 0, k_target, n, L, suffix_sum, self, &mut memo);'
$c = $c -replace 'let need_open: i64 = open_residuals\.iter\(\)\.copied\(\)\.sum\(\)', 'let need_open: i64 = open_residuals.iter().map(|&x| x as i64).sum()'
$c = $c -replace 'new_open\.push\(r_new\)', 'new_open.push(r_new as i32)'
$c = $c -replace 'let mut ol: Vec<i32> = open_list\.iter\(\)\.map\(\|\(r, _\)\| \*r\)\.collect\(\)', 'let mut ol: Vec<i64> = open_list.iter().map(|(r, _)| *r).collect()'
$c = $c -replace 'let mut remaining = L\.saturating_sub\(assigned as u32\) as i32', 'let mut remaining = (L as usize).saturating_sub(assigned as usize) as i32'
$c = $c -replace '\.filter\(\|\(\&s, \&c\)\| s > \*c\)', '.filter(|&(&s, &c)| s > c)'
$c = $c -replace 'SchedulerError::Internal', 'SchedulerError::NotInitialized'
[System.IO.File]::WriteAllText($p, $c, $enc)

Write-Host "Fixing unified_queue.rs..."
$p = "crates/jcode-unified-scheduler/src/unified_queue.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace '\.signed_duration_since\(ts\)', '.signed_duration_since(*ts)'
[System.IO.File]::WriteAllText($p, $c, $enc)

Write-Host "Fixing request_router.rs..."
$p = "crates/jcode-unified-scheduler/src/request_router.rs"
$c = [System.IO.File]::ReadAllText($p, $enc)
$c = $c -replace '\.min_by_key\(\|\(_, \&cost\)\| ordered_float::OrderedFloat\(cost\)\)', '.min_by_key(|(_, cost)| ordered_float::OrderedFloat(*cost))'
[System.IO.File]::WriteAllText($p, $c, $enc)

Write-Host "Done" -ForegroundColor Green
