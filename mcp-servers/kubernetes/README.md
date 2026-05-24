# Kubernetes MCP Server

MCP server for Kubernetes cluster management.

## Features

- **15+ Tools** for K8s operations
- Pod/Deployment/Service management
- Log retrieval and resource monitoring
- Namespace operations
- ConfigMap and Secret management

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `KUBECONFIG` | Path to kubeconfig file | `~/.kube/config` |

## Available Tools

1. `list_pods(namespace = "default")` - List pods in namespace
2. `get_pod_logs(pod_name, namespace)` - Get pod logs
3. `delete_pod(pod_name, namespace)` - Delete a pod
4. `list_deployments(namespace)` - List deployments
5. `scale_deployment(name, namespace, replicas)` - Scale deployment
6. `list_services(namespace)` - List services
7. `get_events(namespace)` - Get cluster events
8. `list_namespaces()` - List all namespaces
9. `create_namespace(name)` - Create namespace
10. `get_resource_usage(namespace)` - Get CPU/memory usage

## Testing

```bash
pytest tests/test_kubernetes_mcp.py -v
```

Requires active K8s cluster connection.

## License

MIT
