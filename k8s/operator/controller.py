#!/usr/bin/env python3
"""
CarpAI Cluster Operator - Kubernetes Controller
Manages CarpAICluster custom resources and automates cluster lifecycle.
"""

import kopf
import kubernetes
from kubernetes import client, config
import logging
import json

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Load kubeconfig
try:
    config.load_incluster_config()
except config.ConfigException:
    config.load_kube_config()

v1 = client.CoreV1Api()
apps_v1 = client.AppsV1Api()


@kopf.on.create('carpai.io', 'v1alpha1', 'carpaiclusters')
def create_carpai_cluster(spec, name, namespace, **kwargs):
    """Handle creation of CarpAICluster resource"""
    logger.info(f"Creating CarpAI cluster: {name}")

    replicas = spec.get('replicas', 3)
    image = spec.get('image', 'carpai:latest')
    tls_enabled = spec.get('tls', {}).get('enabled', True)
    auth_enabled = spec.get('auth', {}).get('enabled', True)

    # Create StatefulSet for cluster nodes
    statefulset = create_statefulset(
        name=name,
        namespace=namespace,
        replicas=replicas,
        image=image,
        tls_enabled=tls_enabled,
        auth_enabled=auth_enabled,
        spec=spec
    )

    apps_v1.create_namespaced_stateful_set(
        namespace=namespace,
        body=statefulset
    )

    # Create headless service for peer discovery
    service = create_headless_service(name, namespace)
    v1.create_namespaced_service(namespace=namespace, body=service)

    # Create autoscaler if enabled
    if spec.get('autoscaling', {}).get('enabled', False):
        hpa = create_hpa(name, namespace, spec['autoscaling'])
        autoscaling_v2 = client.AutoscalingV2Api()
        autoscaling_v2.create_namespaced_horizontal_pod_autoscaler(
            namespace=namespace,
            body=hpa
        )

    return {'message': f'CarpAI cluster {name} created with {replicas} nodes'}


@kopf.on.update('carpai.io', 'v1alpha1', 'carpaiclusters')
def update_carpai_cluster(spec, name, namespace, **kwargs):
    """Handle updates to CarpAICluster resource"""
    logger.info(f"Updating CarpAI cluster: {name}")

    replicas = spec.get('replicas', 3)

    # Scale the StatefulSet
    apps_v1.patch_namespaced_stateful_set_scale(
        name=name,
        namespace=namespace,
        body={'spec': {'replicas': replicas}}
    )

    return {'message': f'CarpAI cluster {name} scaled to {replicas} nodes'}


@kopf.on.delete('carpai.io', 'v1alpha1', 'carpaiclusters')
def delete_carpai_cluster(name, namespace, **kwargs):
    """Handle deletion of CarpAICluster resource"""
    logger.info(f"Deleting CarpAI cluster: {name}")

    try:
        apps_v1.delete_namespaced_stateful_set(name=name, namespace=namespace)
        v1.delete_namespaced_service(name=name, namespace=namespace)
    except client.ApiException as e:
        if e.status != 404:
            raise

    return {'message': f'CarpAI cluster {name} deleted'}


def create_statefulset(name, namespace, replicas, image, tls_enabled, auth_enabled, spec):
    """Create StatefulSet manifest for CarpAI cluster"""

    # Container environment
    env_vars = [
        {
            'name': 'NODE_ID',
            'valueFrom': {
                'fieldRef': {
                    'fieldPath': 'metadata.name'
                }
            }
        },
        {
            'name': 'POD_NAME',
            'valueFrom': {
                'fieldRef': {
                    'fieldPath': 'metadata.name'
                }
            }
        },
        {
            'name': 'NAMESPACE',
            'valueFrom': {
                'fieldRef': {
                    'fieldPath': 'metadata.namespace'
                }
            }
        },
        {
            'name': 'CLUSTER_MODE',
            'value': 'distributed'
        },
        {
            'name': 'PEER_DISCOVERY',
            'value': 'kubernetes'
        }
    ]

    # Add TLS configuration if enabled
    if tls_enabled:
        env_vars.extend([
            {
                'name': 'TLS_ENABLED',
                'value': 'true'
            },
            {
                'name': 'TLS_CERT_PATH',
                'value': '/etc/tls/tls.crt'
            },
            {
                'name': 'TLS_KEY_PATH',
                'value': '/etc/tls/tls.key'
            }
        ])

    # Add auth configuration if enabled
    if auth_enabled:
        jwt_secret_ref = spec.get('auth', {}).get('jwtSecretRef', {})
        if jwt_secret_ref:
            env_vars.append({
                'name': 'JWT_SECRET',
                'valueFrom': {
                    'secretKeyRef': {
                        'name': jwt_secret_ref.get('name', 'carpai-jwt-secret'),
                        'key': jwt_secret_ref.get('key', 'secret')
                    }
                }
            })

    # Resource requests/limits
    resources = spec.get('resources', {})
    resource_spec = {
        'requests': resources.get('requests', {'cpu': '250m', 'memory': '256Mi'}),
        'limits': resources.get('limits', {'cpu': '1000m', 'memory': '1Gi'})
    }

    # Volume mounts
    volume_mounts = []
    volumes = []

    if tls_enabled:
        volume_mounts.append({
            'name': 'tls-certs',
            'mountPath': '/etc/tls',
            'readOnly': True
        })
        volumes.append({
            'name': 'tls-certs',
            'secret': {
                'secretName': spec.get('tls', {}).get('secretName', 'carpai-tls-secret')
            }
        })

    container = {
        'name': 'carpai',
        'image': image,
        'ports': [
            {'containerPort': 8080, 'name': 'http'},
            {'containerPort': 9000, 'name': 'grpc'},
            {'containerPort': 9090, 'name': 'metrics'}
        ],
        'env': env_vars,
        'resources': resource_spec,
        'volumeMounts': volume_mounts,
        'livenessProbe': {
            'httpGet': {
                'path': '/api/health',
                'port': 8080
            },
            'initialDelaySeconds': 30,
            'periodSeconds': 10
        },
        'readinessProbe': {
            'httpGet': {
                'path': '/api/ready',
                'port': 8080
            },
            'initialDelaySeconds': 10,
            'periodSeconds': 5
        }
    }

    # Prometheus annotations
    monitoring = spec.get('monitoring', {})
    annotations = {}
    if monitoring.get('enabled', True):
        annotations = {
            'prometheus.io/scrape': 'true',
            'prometheus.io/port': str(monitoring.get('metricsPort', 9090)),
            'prometheus.io/path': '/metrics'
        }

    statefulset = {
        'apiVersion': 'apps/v1',
        'kind': 'StatefulSet',
        'metadata': {
            'name': name,
            'namespace': namespace,
            'labels': {
                'app.kubernetes.io/name': 'carpai',
                'app.kubernetes.io/component': 'cluster',
                'carpai.io/cluster': name
            }
        },
        'spec': {
            'serviceName': name,
            'replicas': replicas,
            'selector': {
                'matchLabels': {
                    'app.kubernetes.io/name': 'carpai',
                    'carpai.io/cluster': name
                }
            },
            'template': {
                'metadata': {
                    'labels': {
                        'app.kubernetes.io/name': 'carpai',
                        'carpai.io/cluster': name
                    },
                    'annotations': annotations
                },
                'spec': {
                    'containers': [container],
                    'volumes': volumes
                }
            }
        }
    }

    return statefulset


def create_headless_service(name, namespace):
    """Create headless service for peer discovery"""
    service = {
        'apiVersion': 'v1',
        'kind': 'Service',
        'metadata': {
            'name': name,
            'namespace': namespace,
            'labels': {
                'app.kubernetes.io/name': 'carpai',
                'carpai.io/cluster': name
            }
        },
        'spec': {
            'clusterIP': 'None',  # Headless service
            'ports': [
                {'port': 8080, 'name': 'http'},
                {'port': 9000, 'name': 'grpc'},
                {'port': 9090, 'name': 'metrics'}
            ],
            'selector': {
                'app.kubernetes.io/name': 'carpai',
                'carpai.io/cluster': name
            }
        }
    }
    return service


def create_hpa(name, namespace, autoscaling_spec):
    """Create HorizontalPodAutoscaler"""
    min_replicas = autoscaling_spec.get('minReplicas', 3)
    max_replicas = autoscaling_spec.get('maxReplicas', 20)
    target_cpu = autoscaling_spec.get('targetCPUUtilization', 70)
    target_memory = autoscaling_spec.get('targetMemoryUtilization', 80)

    hpa = {
        'apiVersion': 'autoscaling/v2',
        'kind': 'HorizontalPodAutoscaler',
        'metadata': {
            'name': f'{name}-hpa',
            'namespace': namespace
        },
        'spec': {
            'scaleTargetRef': {
                'apiVersion': 'apps/v1',
                'kind': 'StatefulSet',
                'name': name
            },
            'minReplicas': min_replicas,
            'maxReplicas': max_replicas,
            'metrics': [
                {
                    'type': 'Resource',
                    'resource': {
                        'name': 'cpu',
                        'target': {
                            'type': 'Utilization',
                            'averageUtilization': target_cpu
                        }
                    }
                },
                {
                    'type': 'Resource',
                    'resource': {
                        'name': 'memory',
                        'target': {
                            'type': 'Utilization',
                            'averageUtilization': target_memory
                        }
                    }
                }
            ],
            'behavior': {
                'scaleUp': {
                    'stabilizationWindowSeconds': 60,
                    'policies': [
                        {'type': 'Percent', 'value': 50, 'periodSeconds': 60},
                        {'type': 'Pods', 'value': 2, 'periodSeconds': 60}
                    ],
                    'selectPolicy': 'Max'
                },
                'scaleDown': {
                    'stabilizationWindowSeconds': 300,
                    'policies': [
                        {'type': 'Percent', 'value': 10, 'periodSeconds': 60},
                        {'type': 'Pods', 'value': 1, 'periodSeconds': 60}
                    ],
                    'selectPolicy': 'Min'
                }
            }
        }
    }
    return hpa


if __name__ == '__main__':
    kopf.run()
