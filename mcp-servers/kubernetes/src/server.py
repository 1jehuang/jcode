"""
Kubernetes MCP Server
=====================
Provides: Pod management, Deployment operations, Cluster inspection, Config/Secret viewing
Uses FastMCP and the official kubernetes Python client.
"""

from mcp.server import FastMCP
from kubernetes import client, config
from kubernetes.client.rest import ApiException
import os
from typing import Optional

mcp = FastMCP("kubernetes")


def _get_kubeconfig_path() -> Optional[str]:
    return os.environ.get("KUBECONFIG") or None


def _init_clients():
    kubeconfig_path = _get_kubeconfig_path()
    try:
        if kubeconfig_path:
            config.load_kube_config(config_file=kubeconfig_path)
        else:
            config.load_kube_config()
    except Exception as exc:
        try:
            config.load_incluster_config()
        except Exception:
            raise RuntimeError(
                f"Could not load kubernetes configuration. "
                f"Checked KUBECONFIG env, default kubeconfig, and in-cluster config. "
                f"Original error: {exc}"
            )
    return client.CoreV1Api(), client.AppsV1Api()


_core_api: Optional[client.CoreV1Api] = None
_apps_api: Optional[client.AppsV1Api] = None


def _get_core_api() -> client.CoreV1Api:
    global _core_api, _apps_api
    if _core_api is None:
        core, apps = _init_clients()
        _core_api = core
        _apps_api = apps
    return _core_api


def _get_apps_api() -> client.AppsV1Api:
    global _core_api, _apps_api
    if _apps_api is None:
        core, apps = _init_clients()
        _apps_api = apps
        _core_api = core
    return _apps_api


def _format_time(dt) -> str:
    return str(dt) if dt else "N/A"


# ---------------------------------------------------------------------------
# Tool 1: list_pods
# ---------------------------------------------------------------------------
@mcp.tool()
def list_pods(namespace: str = "default") -> str:
    """List all pods in the specified namespace."""
    try:
        api = _get_core_api()
        pods = api.list_namespaced_pod(namespace=namespace)
    except ApiException as e:
        return f"Error listing pods: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not pods.items:
        return f"No pods found in namespace '{namespace}'."

    lines = [f"Pods in namespace '{namespace}':"]
    for pod in pods.items:
        status = pod.status.phase if pod.status else "Unknown"
        restarts = sum(
            cs.restart_count for cs in (pod.status.container_statuses or [])
        )
        ip = pod.status.pod_ip if pod.status else "N/A"
        created = _format_time(pod.metadata.creation_timestamp)
        lines.append(f"  - {pod.metadata.name} | Status: {status} | IP: {ip} | Restarts: {restarts} | Created: {created}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 2: get_pod_logs
# ---------------------------------------------------------------------------
@mcp.tool()
def get_pod_logs(namespace: str, pod_name: str, tail_lines: int = 100) -> str:
    """Get logs from a pod. Optionally limit to the last N lines."""
    try:
        api = _get_core_api()
        logs = api.read_namespaced_pod_log(
            name=pod_name,
            namespace=namespace,
            tail_lines=tail_lines,
        )
        return logs or f"No logs returned for pod '{pod_name}'."
    except ApiException as e:
        return f"Error getting logs: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"


# ---------------------------------------------------------------------------
# Tool 3: restart_deployment
# ---------------------------------------------------------------------------
@mcp.tool()
def restart_deployment(namespace: str, deployment: str) -> str:
    """Rollout restart a deployment."""
    try:
        api = _get_apps_api()
        body = {
            "spec": {
                "template": {
                    "metadata": {
                        "annotations": {
                            "kubectl.kubernetes.io/restartedAt": str(
                                __import__("datetime").datetime.now()
                            )
                        }
                    }
                }
            }
        }
        api.patch_namespaced_deployment(
            name=deployment, namespace=namespace, body=body
        )
        return f"Deployment '{deployment}' in namespace '{namespace}' rolled out (restart annotation added)."
    except ApiException as e:
        return f"Error restarting deployment: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"


# ---------------------------------------------------------------------------
# Tool 4: get_deployments
# ---------------------------------------------------------------------------
@mcp.tool()
def get_deployments(namespace: str = "default") -> str:
    """List deployments in the specified namespace."""
    try:
        api = _get_apps_api()
        deps = api.list_namespaced_deployment(namespace=namespace)
    except ApiException as e:
        return f"Error listing deployments: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not deps.items:
        return f"No deployments found in namespace '{namespace}'."

    lines = [f"Deployments in namespace '{namespace}':"]
    for dep in deps.items:
        ready = f"{dep.status.ready_replicas or 0}/{dep.status.replicas or 0}"
        image = ""
        if dep.spec.template.spec.containers:
            image = dep.spec.template.spec.containers[0].image
        lines.append(f"  - {dep.metadata.name} | Replicas: {ready} | Image: {image}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 5: get_services
# ---------------------------------------------------------------------------
@mcp.tool()
def get_services(namespace: str = "default") -> str:
    """List services in the specified namespace."""
    try:
        api = _get_core_api()
        svcs = api.list_namespaced_service(namespace=namespace)
    except ApiException as e:
        return f"Error listing services: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not svcs.items:
        return f"No services found in namespace '{namespace}'."

    lines = [f"Services in namespace '{namespace}':"]
    for svc in svcs.items:
        svc_type = svc.spec.type
        cluster_ip = svc.spec.cluster_ip or "None"
        ports = ", ".join(
            f"{p.port}/{p.protocol or 'TCP'}" for p in (svc.spec.ports or [])
        )
        lines.append(f"  - {svc.metadata.name} | Type: {svc_type} | ClusterIP: {cluster_ip} | Ports: [{ports}]")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 6: get_namespaces
# ---------------------------------------------------------------------------
@mcp.tool()
def get_namespaces() -> str:
    """List all namespaces in the cluster."""
    try:
        api = _get_core_api()
        nss = api.list_namespace()
    except ApiException as e:
        return f"Error listing namespaces: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    lines = ["Namespaces:"]
    for ns in nss.items:
        status = ns.status.phase if ns.status else "Active"
        lines.append(f"  - {ns.metadata.name} ({status})")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 7: get_nodes
# ---------------------------------------------------------------------------
@mcp.tool()
def get_nodes() -> str:
    """List all nodes in the cluster."""
    try:
        api = _get_core_api()
        nodes = api.list_node()
    except ApiException as e:
        return f"Error listing nodes: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not nodes.items:
        return "No nodes found."

    lines = ["Cluster nodes:"]
    for node in nodes.items:
        status = "Ready"
        for cond in (node.status.conditions or []):
            if cond.type == "Ready":
                status = "Ready" if cond.status == "True" else "NotReady"
        version = node.status.node_info.kubelet_version if node.status.node_info else "N/A"
        lines.append(f"  - {node.metadata.name} | Status: {status} | Kubelet: {version}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 8: describe_pod
# ---------------------------------------------------------------------------
@mcp.tool()
def describe_pod(namespace: str, pod_name: str) -> str:
    """Describe a pod with detailed information."""
    try:
        api = _get_core_api()
        pod = api.read_namespaced_pod(name=pod_name, namespace=namespace)
    except ApiException as e:
        return f"Error describing pod: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    lines = [f"Pod: {pod.metadata.name}"]
    lines.append(f"Namespace: {pod.metadata.namespace}")
    lines.append(f"Status: {pod.status.phase}")
    lines.append(f"Node: {pod.spec.node_name or 'N/A'}")
    lines.append(f"IP: {pod.status.pod_ip or 'N/A'}")
    lines.append(f"Created: {_format_time(pod.metadata.creation_timestamp)}")

    if pod.spec.containers:
        lines.append("\nContainers:")
        for c in pod.spec.containers:
            lines.append(f"  - {c.name} | Image: {c.image}")
            if c.resources:
                req = c.resources.requests or {}
                lim = c.resources.limits or {}
                lines.append(f"    Requests: CPU={req.get('cpu','-')} Mem={req.get('memory','-')}")
                lines.append(f"    Limits:   CPU={lim.get('cpu','-')} Mem={lim.get('memory','-')}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 9: get_events
# ---------------------------------------------------------------------------
@mcp.tool()
def get_events(namespace: str = "default") -> str:
    """Get recent Kubernetes events in the specified namespace."""
    try:
        api = _get_core_api()
        events = api.list_namespaced_event(namespace=namespace)
    except ApiException as e:
        return f"Error listing events: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not events.items:
        return f"No events in namespace '{namespace}'."

    lines = [f"Events in namespace '{namespace}':"]
    for ev in sorted(events.items, key=lambda e: e.metadata.creation_timestamp or "", reverse=True)[:50]:
        lines.append(
            f"  - [{ev.type}] {ev.reason}: {ev.message} (involved: {ev.involved_object.kind}/{ev.involved_object.name})"
        )
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 10: scale_deployment
# ---------------------------------------------------------------------------
@mcp.tool()
def scale_deployment(namespace: str, deployment: str, replicas: int) -> str:
    """Scale a deployment to the specified number of replicas."""
    try:
        api = _get_apps_api()
        body = {"spec": {"replicas": replicas}}
        api.patch_namespaced_deployment_scale(
            name=deployment, namespace=namespace, body=body
        )
        return f"Deployment '{deployment}' scaled to {replicas} replicas."
    except ApiException as e:
        return f"Error scaling deployment: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"


# ---------------------------------------------------------------------------
# Tool 11: get_configmaps
# ---------------------------------------------------------------------------
@mcp.tool()
def get_configmaps(namespace: str = "default") -> str:
    """List ConfigMaps in the specified namespace (shows keys, not values)."""
    try:
        api = _get_core_api()
        cms = api.list_namespaced_config_map(namespace=namespace)
    except ApiException as e:
        return f"Error listing ConfigMaps: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not cms.items:
        return f"No ConfigMaps in namespace '{namespace}'."

    lines = [f"ConfigMaps in namespace '{namespace}':"]
    for cm in cms.items:
        keys = list(cm.data.keys()) if cm.data else []
        lines.append(f"  - {cm.metadata.name} | Keys: {', '.join(keys) if keys else '(empty)'}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tool 12: get_secrets
# ---------------------------------------------------------------------------
@mcp.tool()
def get_secrets(namespace: str = "default") -> str:
    """List Secrets in the specified namespace (shows names only, not values)."""
    try:
        api = _get_core_api()
        secrets = api.list_namespaced_secret(namespace=namespace)
    except ApiException as e:
        return f"Error listing Secrets: {e.reason or 'Unknown'} (HTTP {e.status})"
    except RuntimeError as e:
        return f"Configuration error: {e}"

    if not secrets.items:
        return f"No Secrets in namespace '{namespace}'."

    lines = [f"Secrets in namespace '{namespace}' (names only):"]
    for sec in secrets.items:
        stype = sec.type or "Opaque"
        keys = list(sec.data.keys()) if sec.data else []
        lines.append(f"  - {sec.metadata.name} (type: {stype}, keys: {', '.join(keys) if keys else '(empty)'})")
    return "\n".join(lines)


if __name__ == "__main__":
    mcp.run()
