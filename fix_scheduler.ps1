# fix_scheduler.ps1 -- Batch-fix jcode-unified-scheduler Rust 2024 errors
$base = "crates/jcode-unified-scheduler/src"

Write-Host "Fixing Rust 2024 edition errors..." -ForegroundColor Cyan

# 1. Fix goap_planner.rs: op -> operation/operator
Write-Host "  [goap_planner.rs] Fixing field names op -> operation/operator..." -NoNewline
$content = Get-Content "$base/goap_planner.rs" -Raw
# WorldStateEffect: op: EffectOp::Set -> operation: EffectOp::Set
$content = $content -replace '(WorldStateEffect \{ key: "[^"]+".into\(\), )op: EffectOp::Set', '$1operation: EffectOp::Set'
# WorldStateCondition: op: ConditionOp::Equals -> operator: ConditionOp::Equals
$content = $content -replace '(WorldStateCondition \{ key: "[^"]+".into\(\), )op: ConditionOp::Equals', '$1operator: ConditionOp::Equals'
# f64 .min() -> .reduce()
$content = $content -replace '(\.filter\([^)]+\)\s+\.map\([^)]+\)\s+)\.min\(\)', '$1.reduce(f64::min)'
Set-Content "$base/goap_planner.rs" $content
Write-Host "done" -ForegroundColor Green

# 2. Fix lib.rs type mismatches
Write-Host "  [lib.rs] Fixing type mismatches..." -NoNewline
$content = Get-Content "$base/lib.rs" -Raw
$content = $content -replace 'match_simple_task\(&nodes, task\)', 'match_simple_task(&nodes.iter().map(|&n| std::sync::Arc::new(n.clone())).collect::<Vec<_>>(), task)'
$content = $content -replace 'find_optimal_path\(virtual_layers, &nodes\)', 'find_optimal_path(virtual_layers, &nodes.iter().map(|&n| std::sync::Arc::new(n.clone())).collect::<Vec<_>>())'
Set-Content "$base/lib.rs" $content
Write-Host "done" -ForegroundColor Green

Write-Host "Done. Run 'cargo check --package jcode-unified-scheduler 2>&1' to verify." -ForegroundColor Green
