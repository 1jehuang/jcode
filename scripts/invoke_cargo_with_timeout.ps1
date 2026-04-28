param(
    [Parameter(Mandatory=$true)]
    [ValidateNotNullOrEmpty()]
    [string]$Name,

    [Parameter(Mandatory=$true)]
    [ValidateNotNullOrEmpty()]
    [string[]]$CargoArgs,

    [ValidateRange(1, 86400)]
    [int]$TimeoutSeconds = 300
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Stop-ProcessTree {
    param(
        [Parameter(Mandatory=$true)]
        [System.Diagnostics.Process]$Process
    )

    if ($Process.HasExited) {
        return
    }

    $taskkill = Get-Command taskkill.exe -ErrorAction SilentlyContinue
    if ($taskkill) {
        & $taskkill.Source /PID $Process.Id /T /F | ForEach-Object { Write-Host $_ }
        return
    }

    Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
}

$timeoutMilliseconds = $TimeoutSeconds * 1000

Write-Host "::group::$Name"
try {
    Write-Host "cargo $($CargoArgs -join ' ')"
    $process = Start-Process -FilePath 'cargo' -ArgumentList $CargoArgs -NoNewWindow -PassThru

    if (-not $process.WaitForExit($timeoutMilliseconds)) {
        Stop-ProcessTree -Process $process
        throw "$Name timed out after $TimeoutSeconds seconds"
    }

    $exitCode = $process.ExitCode
    if ($exitCode -ne 0) {
        throw "$Name failed with exit code $exitCode"
    }
} finally {
    Write-Host '::endgroup::'
}
