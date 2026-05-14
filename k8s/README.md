# CarpAI Kubernetes Deployment Configuration
# Complete cloud-native deployment setup for production use

## Quick Start

```bash
# Build and push Docker image
docker build -t carpai:latest .
docker tag carpai:latest your-registry/carpai:v1.0.0
docker push your-registry/carpai:v1.0.0

# Deploy to Kubernetes
kubectl apply -f k8s/namespace.yaml
kubectl apply -f k8s/configmap.yaml
kubectl apply -f k8s/deployment.yaml
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/ingress.yaml

# Check deployment status
kubectl get pods -n carpai-system
kubectl logs -n carpai-system -l app=carpai
```

## Architecture

```
┌─────────────────────────────────────────────┐
│              Ingress (nginx)                │
│              Port: 80 / 443                 │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│              Service (ClusterIP)            │
│              Port: 8080                      │
├──────────────────┬──────────────────────────┤
│                  │                          │
│    ┌─────────────▼─────────────┐           │
│    │   Deployment (3 replicas) │           │
│    │   ┌─────┐ ┌─────┐ ┌─────┐ │           │
│    │   │ Pod │ │ Pod │ │ Pod │ │           │
│    │   └─────┘ └─────┘ └─────┘ │           │
│    └─────────────────────────┘           │
│                                          │
│    ┌─────────────────────────┐           │
│    │     ConfigMap            │           │
│    │   (app configuration)    │           │
│    └─────────────────────────┘           │
│                                          │
│    ┌─────────────────────────┐           │
│    │     Secret               │           │
│    │   (API keys, tokens)     │           │
│    └─────────────────────────┘           │
└──────────────────────────────────────────┘
```

## Components

### 1. Namespace
Isolated namespace for all CarpAI resources

### 2. Deployment
- **Replicas**: 3 (configurable)
- **Resource limits**: CPU/Memory requests and limits
- **Health checks**: Liveness and readiness probes
- **Auto-scaling**: HPA support (optional)

### 3. Service
- **Type**: ClusterIP (internal) or LoadBalancer (external)
- **Port**: 8080 (HTTP API)

### 4. Ingress
- **TLS**: Automatic certificate management
- **Routing**: Path-based routing to services

### 5. Monitoring
- **Prometheus**: Metrics scraping
- **Grafana**: Dashboard visualization

## Scaling

```bash
# Horizontal scaling
kubectl scale deployment carpai --replicas=5 -n carpai-system

# Auto-scaling (requires metrics-server)
kubectl autoscale deployment carpai \
  --cpu-percent=80 \
  --min=2 \
  --max=10 \
  -n carpai-system
```

## Monitoring Stack

```bash
# Deploy Prometheus + Grafana
kubectl apply -f k8s/monitoring/
```

Access Grafana at: http://localhost:3000 (port-forward required)
