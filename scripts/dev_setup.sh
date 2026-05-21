#!/bin/bash
# CarpAI Development Environment Setup Script
# This script sets up PostgreSQL and Redis for local development
# Usage: ./scripts/dev_setup.sh [start|stop|restart|status|clean]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
COMPOSE_FILE="$PROJECT_ROOT/docker-compose.yml"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_docker() {
    if ! command -v docker &> /dev/null; then
        log_error "Docker is not installed. Please install Docker Desktop first."
        exit 1
    fi

    if ! docker info &> /dev/null; then
        log_error "Docker daemon is not running. Please start Docker Desktop."
        exit 1
    fi

    if ! command -v docker compose &> /dev/null; then
        log_error "Docker Compose plugin is not installed."
        exit 1
    fi
}

check_env_file() {
    if [ ! -f "$PROJECT_ROOT/.env" ]; then
        log_warning ".env file not found. Creating from .env.example..."
        cp "$PROJECT_ROOT/.env.example" "$PROJECT_ROOT/.env"
        log_info "Please edit .env file and update the configuration values."
        log_warning "IMPORTANT: Change JWT_SECRET to a secure random string!"
    fi
}

start_services() {
    log_info "Starting PostgreSQL and Redis services..."

    cd "$PROJECT_ROOT"
    docker compose --profile dev up -d postgres redis

    log_info "Waiting for services to be healthy..."
    sleep 5

    # Check service health
    local max_retries=30
    local retry_count=0

    while [ $retry_count -lt $max_retries ]; do
        local pg_health=$(docker inspect --format='{{.State.Health.Status}}' carpai-postgres 2>/dev/null || echo "not_found")
        local redis_health=$(docker inspect --format='{{.State.Health.Status}}' carpai-redis 2>/dev/null || echo "not_found")

        if [ "$pg_health" = "healthy" ] && [ "$redis_health" = "healthy" ]; then
            log_success "All services are healthy!"
            print_connection_info
            return 0
        fi

        retry_count=$((retry_count + 1))
        echo -ne "\rWaiting... ($retry_count/$max_retries)"
        sleep 2
    done

    log_error "Services failed to become healthy within timeout."
    docker compose --profile dev logs postgres redis
    exit 1
}

stop_services() {
    log_info "Stopping PostgreSQL and Redis services..."
    cd "$PROJECT_ROOT"
    docker compose --profile dev down
    log_success "Services stopped."
}

restart_services() {
    log_info "Restarting services..."
    stop_services
    start_services
}

show_status() {
    log_info "Service status:"
    cd "$PROJECT_ROOT"
    docker compose --profile dev ps
    echo ""
    log_info "Service logs (last 20 lines):"
    docker compose --profile dev logs --tail=20 postgres redis
}

clean_data() {
    log_warning "This will remove all data in PostgreSQL and Redis volumes!"
    read -p "Are you sure? (yes/no): " confirm
    if [ "$confirm" = "yes" ]; then
        log_info "Cleaning up..."
        cd "$PROJECT_ROOT"
        docker compose --profile dev down -v
        log_success "All data removed."
    else
        log_info "Cancelled."
    fi
}

print_connection_info() {
    echo ""
    log_success "═══════════════════════════════════════════════════"
    log_success "  Development Environment Ready!"
    log_success "═══════════════════════════════════════════════════"
    echo ""
    log_info "PostgreSQL:"
    echo "  Host:     localhost"
    echo "  Port:     5432"
    echo "  Database: carpai"
    echo "  User:     carpai"
    echo "  Password: carpai_dev_password"
    echo "  URL:      postgresql://carpai:carpai_dev_password@localhost:5432/carpai"
    echo ""
    log_info "Redis:"
    echo "  Host: localhost"
    echo "  Port: 6379"
    echo "  URL:  redis://localhost:6379"
    echo ""
    log_info "Useful commands:"
    echo "  Connect to PostgreSQL: docker exec -it carpai-postgres psql -U carpai -d carpai"
    echo "  Connect to Redis:      docker exec -it carpai-redis redis-cli"
    echo "  View logs:             docker compose --profile dev logs -f"
    echo "  Stop services:         ./scripts/dev_setup.sh stop"
    echo ""
    log_success "═══════════════════════════════════════════════════"
}

# Main
case "${1:-start}" in
    start)
        check_docker
        check_env_file
        start_services
        ;;
    stop)
        check_docker
        stop_services
        ;;
    restart)
        check_docker
        restart_services
        ;;
    status)
        check_docker
        show_status
        ;;
    clean)
        check_docker
        clean_data
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status|clean}"
        echo ""
        echo "Commands:"
        echo "  start   - Start PostgreSQL and Redis (default)"
        echo "  stop    - Stop services"
        echo "  restart - Restart services"
        echo "  status  - Show service status and logs"
        echo "  clean   - Remove all data (destructive!)"
        exit 1
        ;;
esac
