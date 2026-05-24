# Docker MCP Server

A Model Context Protocol (MCP) server for Docker container and image management.

## Features

- **12 Tools** for Docker operations
- Container lifecycle management (start/stop/restart/remove)
- Image management (list/pull/remove)
- System information and resource usage
- Log retrieval
- Network and volume operations

## Installation

```bash
pip install -r requirements.txt
```

## Configuration

No environment variables required. Uses Docker socket at `/var/run/docker.sock` (Linux/Mac) or `npipe:////./pipe/docker_engine` (Windows).

## Available Tools

1. `list_containers(all = False)` - List running containers
2. `start_container(container_name)` - Start a container
3. `stop_container(container_name, timeout = 10)` - Stop a container
4. `restart_container(container_name)` - Restart a container
5. `remove_container(container_name, force = False)` - Remove a container
6. `get_logs(container_name, tail = 100)` - Get container logs
7. `list_images()` - List Docker images
8. `pull_image(image_name)` - Pull an image from registry
9. `remove_image(image_name, force = False)` - Remove an image
10. `get_system_info()` - Get Docker system information
11. `get_container_stats(container_name)` - Get real-time stats
12. `exec_command(container_name, command)` - Execute command in container

## Testing

```bash
pytest tests/test_docker_mcp.py -v
```

Note: Tests require Docker daemon running.

## License

MIT
