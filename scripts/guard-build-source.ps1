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

# Files that change the compiled binary WITHOUT being .rs.
#
# The original version of this guard asked git for `*.rs` only, which is wrong:
# DEFAULT_SYSTEM_PROMPT and DEFAULT_SWARM_PROMPT are include_str! of
# prompt/*.md (prompt.rs:7,72), three more prompt assets are include_str! at
# prompt.rs:273-276, and crates/jcode-app-core/build.rs embeds README.md plus
# every docs/*.md into the binary. Editing any of those changes the binary while
# the guard reported "no Rust differs - binary is current" and waved a stale
# build through. That is the exact failure this script exists to prevent, so the
# staleness test has to see what actually gets compiled in.
#
# Test-only fixtures (testdata/*.html, fuzz corpora) are deliberately absent:
# they sit behind #[cfg(test)] and cannot change runtime behaviour, so they do
# not justify a reload.
function Get-BinaryRelevantDiff {
    param([string]$From, [string]$To)

    $pathspecs = @(
        '*.rs'
        'README.md'
        'docs/*.md'
        'crates/jcode-base/src/prompt/*.md'
        'crates/jcode-base/src/prompt/*.txt'
    )

    $found = @()
    foreach ($spec in $pathspecs) {
        $found += @(& git diff --name-only "$From..$To" -- $spec 2>$null)
    }
    return @($found | Where-Object { $_ } | Sort-Object -Unique)
}

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
            $built = $meta.short_hash
            $relevant = Get-BinaryRelevantDiff -From $built -To $shortHead
            if ($relevant.Count -gt 0) {
                [void]$failures.Add("selfdev binary was built from $built but HEAD is $shortHead and $($relevant.Count) compiled file(s) differ; rebuild or the reload ships stale code:")
                foreach ($f in ($relevant | Select-Object -First 10)) { [void]$failures.Add("    $f") }
            } else {
                Write-Output "note: binary was built from $built, HEAD is $shortHead, but nothing compiled into the binary differs"
            }
        }
        if ($meta.dirty) {
            [void]$failures.Add("selfdev binary was built from a dirty tree ($($meta.source_fingerprint)); its code is in no commit")
        }
    }
}

# The file on disk is not the binary in memory. A reload can publish an older
# binary when the sidecar was refreshed after the reload ran, and the file check
# above passes while the server is still serving the previous commit. The server
# writes its own hash next to its socket, so compare that too.
# The server writes its hash as "<socket>.hash" (server.rs, registry_info.socket).
$sock = $env:JCODE_SOCKET
if (-not $sock) { $sock = 'E:\selfdev-tmp\jcode-WorkShop1\jcode.sock' }
$socketHash = "$sock.hash"
if (Test-Path $socketHash) {
    $running = (Get-Content $socketHash -Raw).Trim()
    if ($running -ne $shortHead) {
        # A commit that touches no Rust changes no binary, so the running server
        # is not stale just because HEAD moved. Only demand a rebuild when the
        # code that goes into the binary actually differs.
        $relevant = Get-BinaryRelevantDiff -From $running -To $shortHead
        if ($relevant.Count -gt 0) {
            [void]$failures.Add("the running server reports $running but HEAD is $shortHead and $($relevant.Count) compiled file(s) differ; reload or the session keeps the old code:")
            foreach ($f in ($relevant | Select-Object -First 10)) { [void]$failures.Add("    $f") }
        } else {
            Write-Output "note: server is on $running, HEAD is $shortHead, but nothing compiled into the binary differs - binary is current"
        }
    }
} else {
    [void]$failures.Add("no running-server hash at $socketHash; cannot confirm what is actually executing")
}

if ($failures.Count -gt 0) {
    Write-Output 'BUILD SOURCE GUARD FAILED'
    foreach ($f in $failures) { Write-Output "  - $f" }
    exit 1
}

Write-Output "BUILD SOURCE GUARD OK  branch=$branch head=$shortHead tree=clean binary=current"
exit 0