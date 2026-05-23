#!/bin/bash
# Offline Image Import Script
# Imports Docker images for offline deployment

set -e

echo "========================================="
echo "CarpAI Offline Image Import"
echo "========================================="
echo ""

IMAGE_DIR="${1:-./offline-images}"

if [ ! -d "$IMAGE_DIR" ]; then
    echo "Error: Directory $IMAGE_DIR not found!"
    echo "Usage: bash import_images.sh <path-to-image-directory>"
    exit 1
fi

echo "Importing images from: $IMAGE_DIR"
echo ""

# Import all tar files
for tar_file in "$IMAGE_DIR"/*.tar; do
    if [ -f "$tar_file" ]; then
        filename=$(basename "$tar_file")
        echo "Importing: $filename"
        docker load -i "$tar_file"
        echo "  ✓ Imported"
    fi
done

echo ""
echo "Verifying imported images..."
echo ""

# List imported images
docker images | grep -E "(pgvector|redis|milvus|etcd|minio|higress|jcode)" || true

echo ""
echo "========================================="
echo "Import Complete!"
echo "========================================="
echo ""
echo "Next steps:"
echo "  1. Verify all images are loaded (see list above)"
echo "  2. Deploy with Docker Compose:"
echo "     docker compose --profile enterprise up -d"
echo ""
echo "  Or deploy with Kubernetes:"
echo "     kubectl apply -k kubernetes/overlays/enterprise"
echo ""
