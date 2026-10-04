$ErrorActionPreference = 'SilentlyContinue'
$ProgressPreference = 'SilentlyContinue'

function Complete-Script { exit 0 }

# A failed report must never disturb the session, but it must not vanish
# silently either: the first version of this script swallowed every error and
# that is exactly why a real failure looked like success. Set
# JCODE_HERDR_DEBUG=1 to see what happened.
$debug = $env:JCODE_HERDR_DEBUG -eq '1'
function Trace([string]$m) {
    if ($debug) {
        try {
            $p = Join-Path $env:TEMP 'herdr-jcode-state.log'
            Add-Content -LiteralPath $p -Value ((Get-Date -Format 'HH:mm:ss') + ' ' + $m) -Encoding ascii
        } catch { }
    }
}

Trace ("entered; HERDR_ENV=" + $env:HERDR_ENV)

# Outside Herdr this must do nothing at all.
if ($env:HERDR_ENV -ne '1') { Trace 'not inside herdr, exiting'; Complete-Script }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { Trace 'no HERDR_PANE_ID'; Complete-Script }
if ([string]::IsNullOrWhiteSpace($env:HERDR_SOCKET_PATH)) { Trace 'no HERDR_SOCKET_PATH'; Complete-Script }

# The Herdr docs call the binary plus its CLI "the portable choice, including on
# Windows", so that is the only transport used here.
$bin = $env:HERDR_BIN_PATH
if ([string]::IsNullOrWhiteSpace($bin)) { Trace 'no HERDR_BIN_PATH'; Complete-Script }
if (-not (Test-Path -LiteralPath $bin)) { Trace ("HERDR_BIN_PATH not a file: " + $bin); Complete-Script }

$event = $env:JCODE_HOOK_EVENT
$sessionId = $env:JCODE_HOOK_SESSION_ID
Trace ("event=" + $event + " session=" + $sessionId)

$state = switch ($event) {
    'session_start' { 'working' }
    'turn_start'    { 'working' }
    'turn_end'      { 'idle' }
    'session_end'   { 'idle' }
    default         { $null }
}
if (-not $state) { Trace ('unmapped event ' + $event); Complete-Script }

# Monotonic across turns and restarts. Herdr ignores a report whose seq is not
# higher than the last it accepted, so a per-process counter is not enough; a
# timestamp is, and it survives the process being replaced.
$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()

# Not "herdr:"-prefixed. Herdr reserves that prefix for its own integrations.
$source = 'jcode:jcode-state'

# Report the resume command with the state report so Herdr can reopen this exact
# session into this pane after its own server restarts. Herdr requires the
# source to hold the pane first, so it travels with report-agent.
$resumeArgv = @('jcode', '--resume', $sessionId)

$out = & $bin pane report-agent $env:HERDR_PANE_ID `
    --source $source --agent jcode --state $state --seq $seq `
    --agent-session-id $sessionId -- $resumeArgv 2>&1
$code = $LASTEXITCODE
Trace ("report-agent exit=" + $code + " out=" + (($out | ForEach-Object { $_.ToString() }) -join ' | '))

Complete-Script