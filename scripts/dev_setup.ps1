# CarpAI Development Environment Setup Script (PowerShell)
# This script sets up PostgreSQL and Redis for local development on Windows
# Usage: .\scripts\dev_setup.ps1 [start|stop|restart|status|clean]

param(
    [Parameter(Position=0)]
    [ValidateSet("start", "stop", "restart", "status", "clean")]
    [string]$Action = "start"
)

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ComposeFile = Join-Path $ProjectRoot "docker-compose.yml"

# Functions
function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Cyan
}

function Write-Success {
    param([string]$Message)
    Write-Host "[SUCCESS] $Message" -ForegroundColor Green
}

function Write-Warning-Custom {
    param([string]$Message)
    Write-Host "[WARNING] $Message" -ForegroundColor Yellow
}

function Write-Error-Custom {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
}

function Check-Docker {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        Write-Error-Custom "Docker is not installed. Please install Docker Desktop first."
        exit 1
    }

    try {
        docker info | Out-Null
    } catch {
        Write-Error-Custom "Docker daemon is not running. Please start Docker Desktop."
        exit 1
    }
}

function Check-Env-File {
    $envFile = Join-Path $ProjectRoot ".env"
    if (-not (Test-Path $envFile)) {
        Write-Warning-Custom ".env file not found. Creating from .env.example..."
        Copy-Item (Join-Path $ProjectRoot ".env.example") $envFile
        Write-Info "Please edit .env file and update the configuration values."
        Write-Warning-Custom "IMPORTANT: Change JWT_SECRET to a secure random string!"
    }
}

function Start-Services {
    Write-Info "Starting PostgreSQL and Redis services..."

    Set-Location $ProjectRoot
    docker compose --profile dev up -d postgres redis

    Write-Info "Waiting for services to be healthy..."
    Start-Sleep -Seconds 5

    # Check service health
    $maxRetries = 30
    $retryCount = 0

    while ($retryCount -lt $maxRetries) {
        try {
            $pgHealth = (docker inspect --format='{{.State.Health.Status}}' carpai-postgres 2>$null)
            $redisHealth = (docker inspect --format='{{.State.Health.Status}}' carpai-redis 2>$null)
        } catch {
            $pgHealth = "not_found"
            $redisHealth = "not_found"
        }

        if ($pgHealth -eq "healthy" -and $redisHealth -eq "healthy") {
            Write-Success "All services are healthy!"
            Print-Connection-Info
            return
        }

        $retryCount++
        Write-Host "`rWaiting... ($retryCount/$maxRetries)" -NoNewline
        Start-Sleep -Seconds 2
    }

    Write-Error-Custom "Services failed to become healthy within timeout."
    docker compose --profile dev logs postgres redis
    exit 1
}

function Stop-Services {
    Write-Info "Stopping PostgreSQL and Redis services..."
    Set-Location $ProjectRoot
    docker compose --profile dev down
    Write-Success "Services stopped."
}

function Restart-Services {
    Write-Info "Restarting services..."
    Stop-Services
    Start-Services
}

function Show-Status {
    Write-Info "Service status:"
    Set-Location $ProjectRoot
    docker compose --profile dev ps
    Write-Host ""
    Write-Info "Service logs (last 20 lines):"
    docker compose --profile dev logs --tail=20 postgres redis
}

function Clean-Data {
    Write-Warning-Custom "This will remove all data in PostgreSQL and Redis volumes!"
    $confirm = Read-Host "Are you sure? (yes/no)"
    if ($confirm -eq "yes") {
        Write-Info "Cleaning up..."
        Set-Location $ProjectRoot
        docker compose --profile dev down -v
        Write-Success "All data removed."
    } else {
        Write-Info "Cancelled."
    }
}

function Print-Connection-Info {
    Write-Host ""
    Write-Success "========================================================="
    Write-Success "  Development Environment Ready!"
    Write-Success "========================================================="
    Write-Host ""
    Write-Info "PostgreSQL:"
    Write-Host "  Host:     localhost"
    Write-Host "  Port:     5432"
    Write-Host "  Database: carpai"
    Write-Host "  User:     carpai"
    Write-Host "  Password: carpai_dev_password"
    Write-Host "  URL:      postgresql://carpai:carpai_dev_password@localhost:5432/carpai"
    Write-Host ""
    Write-Info "Redis:"
    Write-Host "  Host: localhost"
    Write-Host "  Port: 6379"
    Write-Host "  URL:  redis://localhost:6379"
    Write-Host ""
    Write-Info "Useful commands:"
    Write-Host "  Connect to PostgreSQL: docker exec -it carpai-postgres psql -U carpai -d carpai"
    Write-Host "  Connect to Redis:      docker exec -it carpai-redis redis-cli"
    Write-Host "  View logs:             docker compose --profile dev logs -f"
    Write-Host "  Stop services:         .\scripts\dev_setup.ps1 stop"
    Write-Host ""
    Write-Success "========================================================="
}

# Main
switch ($Action) {
    "start" {
        Check-Docker
        Check-Env-File
        Start-Services
    }
    "stop" {
        Check-Docker
        Stop-Services
    }
    "restart" {
        Check-Docker
        Restart-Services
    }
    "status" {
        Check-Docker
        Show-Status
    }
    "clean" {
        Check-Docker
        Clean-Data
    }
    default {
        Write-Host "Usage: .\scripts\dev_setup.ps1 [start|stop|restart|status|clean]"
        Write-Host ""
        Write-Host "Commands:"
        Write-Host "  start   - Start PostgreSQL and Redis (default)"
        Write-Host "  stop    - Stop services"
        Write-Host "  restart - Restart services"
        Write-Host "  status  - Show service status and logs"
        Write-Host "  clean   - Remove all data (destructive!)"
        exit 1
    }
}
