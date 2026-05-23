# MCP Server for Docker Integration
# Provides: Container control, Image management, System info

from mcp.server import FastMCP
import docker

mcp = FastMCP("docker")

def get_client():
    return docker.from_env()

@mcp.tool()
async def list_containers(all: bool = False) -> str:
    """List Docker containers"""
    client = get_client()
    containers = client.containers.list(all=all)
    result = []
    for c in containers:
        status = c.status
        name = c.name
        image = c.image.tags[0] if c.image.tags else c.image.short_id
        result.append(f"{name[:20]:20s} {image:30s} {status}")
    return "\n".join(result) if result else "No containers found"

@mcp.tool()
async def start_container(container_name: str) -> str:
    """Start a Docker container"""
    client = get_client()
    container = client.containers.get(container_name)
    container.start()
    return f"Container {container_name} started"

@mcp.tool()
async def stop_container(container_name: str, timeout: int = 10) -> str:
    """Stop a Docker container"""
    client = get_client()
    container = client.containers.get(container_name)
    container.stop(timeout=timeout)
    return f"Container {container_name} stopped"

@mcp.tool()
async def restart_container(container_name: str) -> str:
    """Restart a Docker container"""
    client = get_client()
    container = client.containers.get(container_name)
    container.restart()
    return f"Container {container_name} restarted"

@mcp.tool()
async def remove_container(container_name: str, force: bool = False) -> str:
    """Remove a Docker container"""
    client = get_client()
    container = client.containers.get(container_name)
    container.remove(force=force)
    return f"Container {container_name} removed"

@mcp.tool()
async def get_container_logs(container_name: str, tail: int = 100) -> str:
    """Get logs from a container"""
    client = get_client()
    container = client.containers.get(container_name)
    logs = container.logs(tail=tail).decode("utf-8")
    return logs

@mcp.tool()
async def list_images() -> str:
    """List Docker images"""
    client = get_client()
    images = client.images.list()
    result = []
    for img in images:
        tags = img.tags[0] if img.tags else "<none>"
        size = img.attrs["Size"] / (1024 * 1024)
        result.append(f"{tags:40s} {size:.1f} MB")
    return "\n".join(result) if result else "No images found"

@mcp.tool()
async def pull_image(image_name: str) -> str:
    """Pull a Docker image"""
    client = get_client()
    client.images.pull(image_name)
    return f"Image {image_name} pulled"

@mcp.tool()
async def remove_image(image_name: str, force: bool = False) -> str:
    """Remove a Docker image"""
    client = get_client()
    client.images.remove(image_name, force=force)
    return f"Image {image_name} removed"

@mcp.tool()
async def get_system_info() -> str:
    """Get Docker system information"""
    client = get_client()
    info = client.info()
    return f"""Containers: {info['Containers']} (Running: {info['ContainersRunning']})
Images: {info['Images']}
Server Version: {info['ServerVersion']}
Storage Driver: {info['Driver']}
Operating System: {info['OperatingSystem']}
Total Memory: {info['MemTotal'] / (1024**3):.1f} GB"""

@mcp.tool()
async def prune_system(volumes: bool = False) -> str:
    """Prune unused Docker resources"""
    client = get_client()
    result = client.containers.prune()
    containers_pruned = len(result.get("ContainersDeleted", []))
    result = client.images.prune()
    images_pruned = len(result.get("ImagesDeleted", []))
    msg = f"Pruned {containers_pruned} containers, {images_pruned} images"
    if volumes:
        result = client.volumes.prune()
        volumes_pruned = len(result.get("VolumesDeleted", []))
        msg += f", {volumes_pruned} volumes"
    return msg

if __name__ == "__main__":
    mcp.run()
