# fix-sshd-config.ps1 — restore Windows OpenSSH server after a broken
# DefaultShell line was added.
#
# Run this from an elevated PowerShell on the desktop (RDP into the box,
# right-click Start → "Windows Terminal (Admin)", then:
#
#   Invoke-WebRequest -UseBasicParsing http://<tailnet>/fix-sshd-config.ps1 -OutFile C:\Windows\Temp\fix-sshd-config.ps1
#   powershell -ExecutionPolicy Bypass -File C:\Windows\Temp\fix-sshd-config.ps1
#
# Or paste the contents inline. The script is idempotent — running it again
# after a successful fix is a no-op.

$ErrorActionPreference = 'Stop'

$configPath = 'C:\ProgramData\ssh\sshd_config'
$backupDir  = 'C:\ProgramData\ssh\backups'
$stamp      = Get-Date -Format 'yyyyMMdd-HHmmss'

if (-not (Test-Path $configPath)) {
    Write-Error "sshd_config not found at $configPath — sshd not installed?"
}

# Find the most recent backup
$latestBackup = Get-ChildItem -Path $backupDir -Filter 'sshd_config.*' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

if (-not $latestBackup) {
    Write-Warning "No backups found in $backupDir. Will rewrite DefaultShell to PowerShell."
    $bak = Join-Path $backupDir ("sshd_config.auto-restored-$stamp")
    Copy-Item -Path $configPath -Destination $bak -Force
} else {
    Write-Host "Restoring from $($latestBackup.FullName)"
    $bak = Join-Path $backupDir ("sshd_config.pre-restore-$stamp")
    Copy-Item -Path $configPath -Destination $bak -Force
    Copy-Item -Path $latestBackup.FullName -Destination $configPath -Force
}

# Sanity-check the resulting config; strip any leftover DefaultShell line
$content = Get-Content -Path $configPath -Raw
$content = $content -replace '(?m)^\s*DefaultShell.*$', ''
$content | Set-Content -Path $configPath -NoNewline

# Validate config syntax via sshd -t
$ss = (Get-Service sshd -ErrorAction SilentlyContinue)
if (-not $ss) {
    Write-Error "sshd service not registered."
}
Write-Host "Testing sshd config..."
$test = & 'C:\Windows\System32\OpenSSH\sshd.exe' -t 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Error "sshd -t failed: $test"
}

# Restart the service
Write-Host "Restarting sshd..."
Restart-Service sshd -Force
Start-Sleep -Seconds 2
$status = (Get-Service sshd).Status
Write-Host "sshd status: $status"

if ($status -eq 'Running') {
    Write-Host "OK — sshd is back up. Test from laptop with: ssh desk 'echo OK'"
    exit 0
} else {
    Write-Error "sshd is not running after restart. Status: $status"
    exit 1
}
