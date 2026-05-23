#!/usr/bin/env python3
"""
CarpAI Cluster Operator - Kubernetes Controller
Manages CarpAICluster, RedisCluster, and MilvusCluster custom resources.
"""

import kopf
import kubernetes
from kubernetes import client, config
import logging
import json
import hashlib

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Load kubeconfig
try:
    config.load_incluster_config()
except config.ConfigException:
    config.load_kube_config()

v1 = client.CoreV1Api()
apps_v1 = client.AppsV1Api()
batch_v1 = client.BatchV1Api()


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


# ============================================================================
# Redis Cluster Operator
# ============================================================================

@kopf.on.create('carpai.io', 'v1alpha1', 'redisclusters')
def create_redis_cluster(spec, name, namespace, **kwargs):
    """Handle creation of RedisCluster resource"""
    logger.info(f"Creating Redis cluster: {name}")

    replicas = spec.get('replicas', 6)
    image = spec.get('image', 'redis:7-alpine')
    config = spec.get('config', {})
    resources = spec.get('resources', {})

    # Create StatefulSet for Redis nodes
    statefulset = create_redis_statefulset(
        name=name,
        namespace=namespace,
        replicas=replicas,
        image=image,
        redis_config=config,
        resource_spec=resources,
        spec=spec
    )

    apps_v1.create_namespaced_stateful_set(
        namespace=namespace,
        body=statefulset
    )

    # Create headless service for Redis cluster communication
    service = create_redis_service(name, namespace)
    v1.create_namespaced_service(namespace=namespace, body=service)

    # Create initialization job to set up the cluster
    init_job = create_redis_init_job(name, namespace, replicas)
    batch_v1.create_namespaced_job(namespace=namespace, body=init_job)

    return {'message': f'Redis cluster {name} creation initiated'}


@kopf.on.update('carpai.io', 'v1alpha1', 'redisclusters')
def update_redis_cluster(spec, name, namespace, status, **kwargs):
    """Handle updates to RedisCluster resource"""
    logger.info(f"Updating Redis cluster: {name}")
    # TODO: Implement rolling update logic
    return {'message': f'Redis cluster {name} update in progress'}


@kopf.on.delete('carpai.io', 'v1alpha1', 'redisclusters')
def delete_redis_cluster(name, namespace, **kwargs):
    """Handle deletion of RedisCluster resource"""
    logger.info(f"Deleting Redis cluster: {name}")
    # Kubernetes will clean up StatefulSet and associated resources
    return {'message': f'Redis cluster {name} deletion initiated'}


def create_redis_statefulset(name, namespace, replicas, image, redis_config, resource_spec, spec):
    """Create Redis StatefulSet manifest"""
    storage = spec.get('storage', {})
    storage_size = storage.get('size', '10Gi')
    storage_class = storage.get('storageClass', 'standard')

    maxmemory = redis_config.get('maxmemory', '512mb')
    maxmemory_policy = redis_config.get('maxmemoryPolicy', 'allkeys-lru')
    appendonly = redis_config.get('appendonly', True)
    save_config = redis_config.get('saveConfig', '900 1 300 10 60 10000')

    container_spec = {
        'name': 'redis',
        'image': image,
        'command': [
            'redis-server',
            '--port', '6379',
            '--cluster-enabled', 'yes',
            '--cluster-config-file', 'nodes.conf',
            '--cluster-node-timeout', '5000',
            '--appendonly', str(appendonly).lower(),
            '--maxmemory', maxmemory,
            '--maxmemory-policy', maxmemory_policy,
            '--save', save_config.replace(' ', ';'),
        ],
        'ports': [
            {'containerPort': 6379, 'name': 'redis'},
            {'containerPort': 16379, 'name': 'cluster-bus'},
        ],
        'resources': resource_spec if resource_spec else {
            'requests': {'cpu': '500m', 'memory': '512Mi'},
            'limits': {'cpu': '1', 'memory': '1Gi'},
        },
        'volumeMounts': [
            {'name': 'redis-data', 'mountPath': '/data'},
        ],
        'livenessProbe': {
            'exec': {'command': ['redis-cli', 'ping']},
            'initialDelaySeconds': 30,
            'periodSeconds': 10,
        },
        'readinessProbe': {
            'exec': {'command': ['redis-cli', 'ping']},
            'initialDelaySeconds': 5,
            'periodSeconds': 5,
        },
    }

    statefulset = {
        'apiVersion': 'apps/v1',
        'kind': 'StatefulSet',
        'metadata': {
            'name': name,
            'labels': {
                'app': 'redis-cluster',
                'cluster': name,
            },
        },
        'spec': {
            'serviceName': f'{name}-headless',
            'replicas': replicas,
            'selector': {
                'matchLabels': {
                    'app': 'redis-cluster',
                    'cluster': name,
                },
            },
            'template': {
                'metadata': {
                    'labels': {
                        'app': 'redis-cluster',
                        'cluster': name,
                    },
                },
                'spec': {
                    'containers': [container_spec],
                },
            },
            'volumeClaimTemplates': [
                {
                    'metadata': {'name': 'redis-data'},
                    'spec': {
                        'accessModes': ['ReadWriteOnce'],
                        'storageClassName': storage_class,
                        'resources': {
                            'requests': {'storage': storage_size},
                        },
                    },
                },
            ],
        },
    }

    return statefulset


def create_redis_service(name, namespace):
    """Create headless Service for Redis cluster"""
    service = {
        'apiVersion': 'v1',
        'kind': 'Service',
        'metadata': {
            'name': f'{name}-headless',
            'labels': {
                'app': 'redis-cluster',
                'cluster': name,
            },
        },
        'spec': {
            'clusterIP': 'None',  # Headless service
            'ports': [
                {'port': 6379, 'targetPort': 6379, 'name': 'redis'},
                {'port': 16379, 'targetPort': 16379, 'name': 'cluster-bus'},
            ],
            'selector': {
                'app': 'redis-cluster',
                'cluster': name,
            },
        },
    }
    return service


def create_redis_init_job(name, namespace, replicas):
    """Create Job to initialize Redis cluster"""
    # Build node list for cluster creation command
    nodes = ' '.join([f'{name}-{i}.{name}-headless.{namespace}.svc.cluster.local:6379' for i in range(replicas)])

    job = {
        'apiVersion': 'batch/v1',
        'kind': 'Job',
        'metadata': {
            'name': f'{name}-init',
        },
        'spec': {
            'template': {
                'spec': {
                    'containers': [
                        {
                            'name': 'redis-init',
                            'image': 'redis:7-alpine',
                            'command': [
                                'sh', '-c',
                                f'sleep 10 && redis-cli --cluster create {nodes} --cluster-replicas 1 --cluster-yes'
                            ],
                        },
                    ],
                    'restartPolicy': 'OnFailure',
                },
            },
            'backoffLimit': 5,
        },
    }
    return job


# ============================================================================
# Milvus Cluster Operator
# ============================================================================

@kopf.on.create('carpai.io', 'v1alpha1', 'milvusclusters')
def create_milvus_cluster(spec, name, namespace, **kwargs):
    """Handle creation of MilvusCluster resource"""
    logger.info(f"Creating Milvus cluster: {name}")

    mode = spec.get('mode', 'standalone')
    image = spec.get('image', 'milvusdb/milvus:v2.4.0')

    if mode == 'standalone':
        deployment = create_milvus_standalone(name, namespace, image, spec)
        apps_v1.create_namespaced_deployment(namespace=namespace, body=deployment)
    else:
        # Cluster mode - create multiple components
        create_milvus_cluster_mode(name, namespace, image, spec)

    # Create service
    service = create_milvus_service(name, namespace, mode)
    v1.create_namespaced_service(namespace=namespace, body=service)

    return {'message': f'Milvus cluster {name} creation initiated ({mode})'}


@kopf.on.delete('carpai.io', 'v1alpha1', 'milvusclusters')
def delete_milvus_cluster(name, namespace, **kwargs):
    """Handle deletion of MilvusCluster resource"""
    logger.info(f"Deleting Milvus cluster: {name}")
    return {'message': f'Milvus cluster {name} deletion initiated'}


def create_milvus_standalone(name, namespace, image, spec):
    """Create Milvus standalone deployment"""
    components = spec.get('components', {})
    standalone = components.get('standalone', {})
    resources = standalone.get('resources', {
        'requests': {'cpu': '2', 'memory': '4Gi'},
        'limits': {'cpu': '4', 'memory': '8Gi'},
    })

    storage = spec.get('storage', {})
    storage_size = storage.get('size', '100Gi')

    deployment = {
        'apiVersion': 'apps/v1',
        'kind': 'Deployment',
        'metadata': {
            'name': name,
            'labels': {
                'app': 'milvus',
                'component': 'standalone',
                'cluster': name,
            },
        },
        'spec': {
            'replicas': standalone.get('replicas', 1),
            'selector': {
                'matchLabels': {
                    'app': 'milvus',
                    'cluster': name,
                },
            },
            'template': {
                'metadata': {
                    'labels': {
                        'app': 'milvus',
                        'cluster': name,
                    },
                },
                'spec': {
                    'containers': [
                        {
                            'name': 'milvus',
                            'image': image,
                            'command': ['milvus', 'run', 'standalone'],
                            'ports': [
                                {'containerPort': 19530, 'name': 'rpc'},
                                {'containerPort': 9091, 'name': 'metrics'},
                            ],
                            'env': [
                                {'name': 'ETCD_ENDPOINTS', 'value': f'{name}-etcd:2379'},
                                {'name': 'MINIO_ADDRESS', 'value': f'{name}-minio:9000'},
                            ],
                            'resources': resources,
                            'livenessProbe': {
                                'tcpSocket': {'port': 19530},
                                'initialDelaySeconds': 60,
                                'periodSeconds': 10,
                            },
                            'readinessProbe': {
                                'tcpSocket': {'port': 19530},
                                'initialDelaySeconds': 30,
                                'periodSeconds': 5,
                            },
                        },
                    ],
                },
            },
        },
    }

    return deployment


def create_milvus_service(name, namespace, mode):
    """Create Milvus service"""
    service = {
        'apiVersion': 'v1',
        'kind': 'Service',
        'metadata': {
            'name': name,
            'labels': {
                'app': 'milvus',
                'cluster': name,
            },
        },
        'spec': {
            'type': 'ClusterIP',
            'ports': [
                {'port': 19530, 'targetPort': 19530, 'name': 'rpc'},
                {'port': 9091, 'targetPort': 9091, 'name': 'metrics'},
            ],
            'selector': {
                'app': 'milvus',
                'cluster': name,
            },
        },
    }
    return service


def create_milvus_cluster_mode(name, namespace, image, spec):
    """Create Milvus in cluster mode with multiple components"""
    # TODO: Implement full cluster mode with querynode, datanode, indexnode, proxy
    logger.warning("Milvus cluster mode not fully implemented yet")
    pass


if __name__ == '__main__':
    kopf.run()
