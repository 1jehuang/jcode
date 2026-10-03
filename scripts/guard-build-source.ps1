# Pre-build guard for the self-dev binary.
#
# Publishing a binary built from a different tree than the one you think you are
# on is the failure this exists to prevent. It happened repeatedly in one session
# because every check lived in my head and none of them ran.
#
# Usage: pwsh -File scripts/guard-build-source.ps1
# Exit 0 only when the branch, the working tree and the built binary all agree.

$ErrorActionPreference = 'Stop'
$expectedBranch = 'restore/all-fixes'
$repo = 'E:\jcode'
$failures = @()

function Get-Git([string[]]$args) {
    $out = & git -C $repo @args 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') failed: $out" }
    return ($out | Out-String).Trim()
}

Set-Location $repo

$branch = Get-Git @('rev-parse', '--abbrev-ref', 'HEAD')
$head = Get-Git @('rev-parse', 'HEAD')
$shortHead = Get-Git @('rev-parse', '--short=9', 'HEAD')

$dirty = @(Get-Git @('status', '--porcelain'))
if ($dirty.Count -gt 0) {
    $failures += "working tree has $($dirty.Count) uncommitted change(s); a binary built now would contain code that is not in any commit:"
    $failures += ($dirty | Select-Object -First 10 | ForEach-Object { "    $_" })
}

if ($branch -ne $expectedBranch) {
    $failures += "branch is '$branch', expected '$expectedBranch'. A build on any other branch publishes a tree that is missing the integration commits."
}

# The binary the reload will publish must already be built from HEAD.
$exe = Join-Path $repo 'target\selfdev\jcode.exe'
if (-not (Test-Path $exe)) {
    $failures += "no selfdev binary at $exe"
} else {
    $sidecar = "$exe.source.json"
    if (-not (Test-Path $sidecar)) {
        $failures += "missing $sidecar; run the build before this guard"
    } else {
        $meta = Get-Content $sidecar -Raw | ConvertFrom-Json
        if ($meta.short_hash -ne $shortHead) {
            $failures += "selfdev binary was built from $($meta.short_hash), HEAD is $shortHead. Rebuild before reloading or the reload will ship stale code."
        }
        if ($meta.dirty) {
            $failures += "selfdev binary was built from a dirty tree ($($meta.source_fingerprint)); its code is not reproducible from any commit."
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Output "BUILD SOURCE GUARD FAILED"
    foreach ($f in $failures) { Write-Output "  - $f" }
    exit 1
}

Write-Output "BUILD SOURCE GUARD OK  branch=$branch head=$shortHead tree=clean binary=current"
exit 0