# Pre-build guard for the self-dev binary.
#
# Publishing a binary built from a different tree than the one in use is the
# failure this exists to prevent. It happened repeatedly in one session: a PR
# branch cut off origin/master was built and published while the integration
# branch carried the other fixes, and a stale binary was reloaded twice before
# the reload guard's message was read as an obstacle rather than the answer.
#
# Usage: pwsh -File scripts/guard-build-source.ps1
# Exit 0 only when the branch, the working tree and the built binary all agree.

$ErrorActionPreference = 'Stop'
$expectedBranch = 'restore/all-fixes'
$repo = 'E:\jcode'
$failures = New-Object System.Collections.ArrayList

Set-Location $repo

$branch = (& git rev-parse --abbrev-ref HEAD | Out-String).Trim()
$shortHead = (& git rev-parse --short=9 HEAD | Out-String).Trim()

$dirty = @(& git status --porcelain)
if ($dirty.Count -gt 0 -and -not ($dirty -match '^\s*$' | Measure-Object).Count -eq 0) {
    $real = @($dirty | Where-Object { $_.Trim().Length -gt 0 })
    if ($real.Count -gt 0) {
        [void]$failures.Add("working tree has $($real.Count) uncommitted change(s); a binary built now would ship code that is in no commit:")
        foreach ($line in ($real | Select-Object -First 10)) { [void]$failures.Add("    $line") }
    }
}

if ($branch -ne $expectedBranch) {
    [void]$failures.Add("branch is '$branch', expected '$expectedBranch'; a build on any other branch publishes a tree missing the integration commits")
}

$exe = Join-Path $repo 'target\selfdev\jcode.exe'
if (-not (Test-Path $exe)) {
    [void]$failures.Add("no selfdev binary at $exe")
} else {
    $sidecar = "$exe.source.json"
    if (-not (Test-Path $sidecar)) {
        [void]$failures.Add("missing $sidecar; run the build before this guard")
    } else {
        $meta = Get-Content $sidecar -Raw | ConvertFrom-Json
        if ($meta.short_hash -ne $shortHead) {
            [void]$failures.Add("selfdev binary was built from $($meta.short_hash) but HEAD is $shortHead; rebuild or the reload ships stale code")
        }
        if ($meta.dirty) {
            [void]$failures.Add("selfdev binary was built from a dirty tree ($($meta.source_fingerprint)); its code is in no commit")
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Output 'BUILD SOURCE GUARD FAILED'
    foreach ($f in $failures) { Write-Output "  - $f" }
    exit 1
}

Write-Output "BUILD SOURCE GUARD OK  branch=$branch head=$shortHead tree=clean binary=current"
exit 0