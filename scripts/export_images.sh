#!/bin/bash
# Offline Image Export Script
# Exports all required Docker images for offline deployment

set -e

echo "========================================="
echo "CarpAI Offline Image Export"
echo "========================================="
echo ""

# Output directory
OUTPUT_DIR="${1:-./offline-images}"
mkdir -p "$OUTPUT_DIR"

# List of required images
IMAGES=(
    "pgvector/pgvector:pg15"
    "redis:7-alpine"
    "milvusdb/milvus:v2.3.5"
    "quay.io/coreos/etcd:v3.5.9"
    "minio/minio:RELEASE.2023-03-20T20-16-18Z"
    "higress-registry.cn-hangzhou.cr.aliyuncs.com/higress/higress:v1.3.3"
)

# Optional monitoring images
MONITORING_IMAGES=(
    "otel/opentelemetry-collector-contrib:latest"
    "prom/prometheus:latest"
    "grafana/grafana:latest"
)

echo "Exporting core images..."
for image in "${IMAGES[@]}"; do
    echo "  Pulling and exporting: $image"
    docker pull "$image" || {
        echo "    Warning: Failed to pull $image, trying local cache..."
    }

    # Sanitize image name for filename
    filename=$(echo "$image" | sed 's/[\/:]/_/g')
    docker save -o "$OUTPUT_DIR/${filename}.tar" "$image"
    echo "    ✓ Exported to ${filename}.tar"
done

echo ""
echo "Exporting optional monitoring images (skip if not needed)..."
for image in "${MONITORING_IMAGES[@]}"; do
    echo "  Pulling and exporting: $image"
    docker pull "$image" || {
        echo "    Warning: Failed to pull $image, skipping..."
        continue
    }

    filename=$(echo "$image" | sed 's/[\/:]/_/g')
    docker save -o "$OUTPUT_DIR/${filename}.tar" "$image"
    echo "    ✓ Exported to ${filename}.tar"
done

# Export JCode server image if built locally
echo ""
echo "Checking for local jcode:latest image..."
if docker images | grep -q "jcode.*latest"; then
    echo "  Exporting jcode:latest..."
    docker save -o "$OUTPUT_DIR/jcode_latest.tar" jcode:latest
    echo "    ✓ Exported to jcode_latest.tar"
else
    echo "  ⚠ jcode:latest not found. Build it first with: cargo build --release && docker build -t jcode:latest ."
fi

# Create manifest file
echo ""
echo "Creating image manifest..."
cat > "$OUTPUT_DIR/manifest.txt" <<EOF
# CarpAI Offline Deployment Image Manifest
# Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Core Images
$(printf "%s\n" "${IMAGES[@]}")

## Optional Monitoring Images
$(printf "%s\n" "${MONITORING_IMAGES[@]}")

## Local Images
jcode:latest

## Import Instructions
# On the target machine, run:
#   bash import_images.sh <path-to-this-directory>
EOF

echo "✓ Manifest created: $OUTPUT_DIR/manifest.txt"

# Calculate total size
TOTAL_SIZE=$(du -sh "$OUTPUT_DIR" | cut -f1)
echo ""
echo "========================================="
echo "Export Complete!"
echo "========================================="
echo "Output directory: $OUTPUT_DIR"
echo "Total size: $TOTAL_SIZE"
echo ""
echo "To deploy offline:"
echo "  1. Copy $OUTPUT_DIR to target machine"
echo "  2. Run: bash scripts/import_images.sh $OUTPUT_DIR"
echo "  3. Deploy with: docker compose up -d"
echo ""
