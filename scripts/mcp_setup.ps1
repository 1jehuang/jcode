# CarpAI MCP Server Setup Script (Windows PowerShell)
# Installs dependencies for MCP servers

param(
    [switch]$All,
    [string[]]$Servers
)

$MCPDir = Join-Path $PSScriptRoot ".." "mcp-servers" | Resolve-Path
$PythonCmd = "python"

Write-Host "=== CarpAI MCP Setup ===" -ForegroundColor Cyan
Write-Host "MCP Directory: $MCPDir" -ForegroundColor Gray
Write-Host ""

# Check Python
try {
    $version = & $PythonCmd --version 2>&1
    Write-Host "Python: $version" -ForegroundColor Green
} catch {
    Write-Host "ERROR: python not found. Please install Python 3.10+" -ForegroundColor Red
    exit 1
}

# Install base dependencies
Write-Host "Installing base MCP dependencies..." -ForegroundColor Yellow
& $PythonCmd -m pip install -r "$MCPDir\requirements.txt"

if ($LASTEXITCODE -ne 0) {
    Write-Host "Failed to install base dependencies" -ForegroundColor Red
    exit 1
}

# Function to install server-specific requirements
function Install-ServerRequirements {
    param([string]$Name, [string[]]$EnvVars)

    $missingVars = @()
    foreach ($var in $EnvVars) {
        if (-not (Get-Item "env:$var" -ErrorAction SilentlyContinue)) {
            $missingVars += $var
        }
    }

    if ($missingVars.Count -gt 0) {
        Write-Host "[SKIP] $Name : missing environment variables: $($missingVars -join ', ')" -ForegroundColor DarkYellow
        return
    }

    $reqFile = Join-Path $MCPDir "requirements-$Name.txt"
    if (Test-Path $reqFile) {
        Write-Host "[INSTALL] $Name dependencies..." -ForegroundColor Yellow
        & $PythonCmd -m pip install -r $reqFile
    }
}

# Determine which servers to install
if ($All -or $Servers.Count -eq 0) {
    Install-ServerRequirements "github" @("GITHUB_TOKEN")
    Install-ServerRequirements "jira" @("JIRA_URL", "JIRA_API_TOKEN")
    Install-ServerRequirements "slack" @("SLACK_BOT_TOKEN")
    Install-ServerRequirements "postgres" @("DATABASE_URL")
    Install-ServerRequirements "redis" @("REDIS_URL")
    Install-ServerRequirements "kubernetes" @("KUBECONFIG")
    Install-ServerRequirements "aws" @("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY")
    Install-ServerRequirements "sentry" @("SENTRY_TOKEN")
    Install-ServerRequirements "datadog" @("DATADOG_API_KEY", "DATADOG_APP_KEY")
} else {
    # Install Docker always
    & $PythonCmd -m pip install -r "$MCPDir\requirements-docker.txt"

    foreach ($server in $Servers) {
        switch ($server.ToLower()) {
            "github" { Install-ServerRequirements "github" @("GITHUB_TOKEN") }
            "jira" { Install-ServerRequirements "jira" @("JIRA_URL", "JIRA_API_TOKEN") }
            "slack" { Install-ServerRequirements "slack" @("SLACK_BOT_TOKEN") }
            "postgres" { Install-ServerRequirements "postgres" @("DATABASE_URL") }
            "redis" { Install-ServerRequirements "redis" @("REDIS_URL") }
            "kubernetes" { Install-ServerRequirements "kubernetes" @("KUBECONFIG") }
            "aws" { Install-ServerRequirements "aws" @("AWS_ACCESS_KEY_ID") }
            "sentry" { Install-ServerRequirements "sentry" @("SENTRY_TOKEN") }
            "datadog" { Install-ServerRequirements "datadog" @("DATADOG_API_KEY") }
            default {
                Write-Host "[SKIP] Unknown server: $server" -ForegroundColor DarkYellow
            }
        }
    }
}

Write-Host ""
Write-Host "=== Setup complete ===" -ForegroundColor Green
Write-Host "Run 'python mcp-servers\start_all.py' to start configured servers" -ForegroundColor Cyan
