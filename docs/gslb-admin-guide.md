# GSLB & Cross-Region Sync Administration Guide

## Overview

CarpAI now includes Global Server Load Balancing (GSLB) and cross-region synchronization capabilities for enterprise multi-region deployments.

## Quick Start

### 1. GPU Load Balancer (Auto-Enabled)

The GPU load balancer is **automatically activated** when NVIDIA GPUs are detected:

```bash
# Start CarpAI server - GPU detection happens automatically
jcode server start

# Check logs for GPU detection
# Look for: "Detected X GPU(s), initializing GPU load balancer"
```

**No manual configuration needed!** The system will:
- Auto-detect NVIDIA GPUs via NVML
- Initialize balanced scheduling strategy
- Export metrics to Prometheus every 10 seconds

**Supported GPU Features:**
- NVLink topology awareness
- NUMA-aware scheduling
- Power/thermal monitoring
- Dynamic load balancing (balanced/latency/throughput/power modes)

---

### 2. GSLB Management (Multi-Region Deployment)

For organizations deploying CarpAI across multiple geographic regions:

#### Register Regional Clusters

```bash
# Register a cluster in US East
jcode admin gslb register \
  --cluster-id us-east-prod \
  --region us-east-1 \
  --endpoint https://carpai-us-east.example.com \
  --weight 100

# Register a cluster in Asia Pacific
jcode admin gslb register \
  --cluster-id ap-southeast-prod \
  --region ap-southeast-1 \
  --endpoint https://carpai-ap.example.com \
  --weight 80
```

#### Configure Routing Strategy

```bash
# Recommended: Latency-based routing (best user experience)
jcode admin gslb strategy --strategy latency

# Alternative strategies:
jcode admin gslb strategy --strategy geo          # Geographic proximity
jcode admin gslb strategy --strategy weighted     # Weighted distribution
jcode admin gslb strategy --strategy least-loaded # Least loaded region
jcode admin gslb strategy --strategy failover     # Primary/backup mode
```

#### Monitor Cluster Health

```bash
# View all regional clusters
jcode admin gslb status

# Update health status (for maintenance)
jcode admin gslb health \
  --cluster-id us-east-prod \
  --status maintenance
```

---

### 3. Cross-Region Data Synchronization

Enable automatic session state replication across regions:

```bash
# Start sync on local node
jcode admin gslb sync-start \
  --local-region us-east-1 \
  --local-node node-001 \
  --interval-ms 5000

# View sync statistics
jcode admin gslb sync-stats

# Stop sync (if needed)
jcode admin gslb sync-stop
```

**Synchronization Features:**
- **CRDT-based**: Conflict-free replicated data types
- **Anti-entropy gossip**: Efficient state convergence
- **LWW resolution**: Last-Writer-Wins conflict resolution
- **Session replication**: Automatic user session state sync

---

## Architecture

### GPU Load Balancing Flow

```
User Request → UnifiedScheduler → GPU Discovery (NVML) → Load Balancer → GPU Node
                                      ↓
                                 Topology Awareness
                                 (NVLink/NUMA)
```

### Cross-Region Replication Flow

```
Region A (us-east-1) ←→ Gossip Protocol ←→ Region B (ap-southeast-1)
       ↓                                          ↓
  CRDT Merge                                  CRDT Merge
       ↓                                          ↓
  Local State                                Remote State
```

---

## Monitoring

### Prometheus Metrics

GPU metrics are exported automatically (every 10s):

```prometheus
carpai_gpu_total                  # Total GPU count
carpai_gpu_active                 # Active GPUs
carpai_gpu_avg_utilization        # Average utilization %
carpai_gpu_vram_total_bytes       # Total VRAM
carpai_gpu_vram_used_bytes        # Used VRAM
carpai_gpu_vram_usage_percent     # VRAM usage %
carpai_gpu_pending_requests       # Queued requests
```

### Grafana Dashboard

Import the provided dashboard JSON for visualization:
- GPU utilization over time
- VRAM usage trends
- Cross-region latency heatmap
- Sync conflict rates

---

## Troubleshooting

### GPU Not Detected

```bash
# Check NVML availability
nvidia-smi

# Verify feature flag
cargo build --features gpu-discovery

# Check logs
grep "GPU detection" ~/.jcode/logs/*.log
```

### Cross-Region Sync Issues

```bash
# Check connectivity between regions
ping <remote-cluster-endpoint>

# Verify gossip protocol
jcode admin gslb sync-stats

# Check for conflicts
grep "conflict" ~/.jcode/logs/*.log
```

---

## Configuration Reference

### GPU Balance Strategies

| Strategy | Use Case | Description |
|----------|----------|-------------|
| `balanced` | Default | Balance latency and throughput |
| `latency` | Real-time apps | Minimize response time |
| `throughput` | Batch processing | Maximize total work done |
| `power` | Energy-efficient | Minimize power consumption |

### Routing Strategies

| Strategy | Best For | Behavior |
|----------|----------|----------|
| `latency` | General use | Route to lowest latency region |
| `geo` | Compliance | Route by geographic proximity |
| `weighted` | Capacity mgmt | Distribute by configured weights |
| `least-loaded` | Burst traffic | Route to least busy region |
| `failover` | DR scenarios | Primary/backup only |

---

## API Integration

For programmatic access, use the CarpAI SDK:

```python
import carpai_sdk

# Get GPU status
gpu_stats = client.get_gpu_stats()
print(f"Active GPUs: {gpu_stats.active_gpus}")

# Manage GSLB
client.gslb.register_cluster(
    cluster_id="eu-west-prod",
    region="eu-west-1",
    endpoint="https://carpai-eu.example.com"
)

# Configure routing
client.gslb.set_strategy("latency")
```

---

## Next Steps

1. **Single Region**: GPU load balancing works automatically
2. **Multi-Region**: Follow the GSLB setup guide above
3. **Monitoring**: Set up Prometheus + Grafana dashboards
4. **Testing**: Use `jcode admin gslb status` to verify configuration

For enterprise support, contact your CarpAI account manager.
