# CarpAI 分布式集群部署指南

本指南介绍如何部署具备真实gRPC通信、TLS加密、身份验证和自动扩缩容的CarpAI分布式集群。

---

## 📋 前置要求

- Kubernetes 1.24+
- Helm 3.8+ (可选)
- cert-manager (用于TLS证书管理)
- Prometheus Operator (可选，用于监控)

---

## 🚀 快速开始

### 1. 安装Cert-Manager (TLS证书管理)

```bash
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/v1.13.0/cert-manager.yaml
```

### 2. 创建CA证书和密钥

```bash
# 生成自签名CA
openssl req -x509 -newkey rsa:4096 -keyout ca.key -out ca.crt -days 365 -nodes \
  -subj "/CN=CarpAI CA"

# 创建K8s Secret
kubectl create secret generic carpai-ca-secret \
  --from-file=tls.key=ca.key \
  --from-file=tls.crt=ca.crt \
  -n carpai-system
```

### 3. 部署CRD和Operator

```bash
# 应用CRD
kubectl apply -f k8s/operator/carpai-operator.yaml

# 部署Operator控制器
kubectl apply -f k8s/operator/operator-deployment.yaml
```

### 4. 创建CarpAI集群

```bash
# 应用集群配置
kubectl apply -f k8s/examples/cluster-example.yaml
```

### 5. 验证部署

```bash
# 查看集群状态
kubectl get carpaiclusters -n carpai-system

# 查看Pod状态
kubectl get pods -n carpai-system -l app.kubernetes.io/name=carpai

# 查看服务
kubectl get svc -n carpai-system
```

---

## 🔧 配置选项

### 基础集群配置

```yaml
apiVersion: carpai.io/v1alpha1
kind: CarpAICluster
metadata:
  name: my-carpai-cluster
  namespace: carpai-system
spec:
  replicas: 3
  image: carpai:latest
```

### 启用TLS加密

```yaml
spec:
  tls:
    enabled: true
    secretName: carpai-tls-secret
```

### 启用JWT身份验证

```yaml
spec:
  auth:
    enabled: true
    jwtSecretRef:
      name: carpai-jwt-secret
      key: secret
```

### 资源配置

```yaml
spec:
  resources:
    requests:
      cpu: "500m"
      memory: "512Mi"
    limits:
      cpu: "2000m"
      memory: "2Gi"
```

### 自动扩缩容

```yaml
spec:
  autoscaling:
    enabled: true
    minReplicas: 3
    maxReplicas: 20
    targetCPUUtilization: 70
    targetMemoryUtilization: 80
```

### 监控集成

```yaml
spec:
  monitoring:
    enabled: true
    prometheusScrape: true
    metricsPort: 9090
```

---

## 🔐 安全配置

### 1. TLS证书配置

使用cert-manager自动管理证书：

```yaml
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: carpai-tls-cert
  namespace: carpai-system
spec:
  secretName: carpai-tls-secret
  duration: 2160h  # 90天
  renewBefore: 360h  # 提前15天续期
  dnsNames:
    - "*.carpai-system.svc.cluster.local"
  issuerRef:
    name: carpai-ca-issuer
    kind: ClusterIssuer
```

### 2. JWT身份验证

创建JWT密钥：

```bash
# 生成JWT密钥
export JWT_SECRET=$(openssl rand -hex 32)

# 创建Secret
kubectl create secret generic carpai-jwt-secret \
  --from-literal=secret=$JWT_SECRET \
  -n carpai-system
```

节点间通信将自动携带JWT令牌进行身份验证。

### 3. 网络策略

限制集群内部通信：

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: carpai-network-policy
  namespace: carpai-system
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: carpai
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - podSelector:
            matchLabels:
              app.kubernetes.io/name: carpai
      ports:
        - port: 9000  # gRPC
        - port: 9090  # Metrics
  egress:
    - to:
        - podSelector:
            matchLabels:
              app.kubernetes.io/name: carpai
      ports:
        - port: 9000
        - port: 9090
```

---

## 📊 监控和可观测性

### Prometheus指标

集群暴露以下Prometheus指标：

```promql
# 集群规模
cluster_size

# 健康节点数
cluster_healthy_nodes

# 领导者状态
cluster_is_leader

# 选举次数
rate(cluster_elections_initiated_total[5m])

# 心跳失败率
rate(cluster_failed_heartbeats_total[5m])

# 任务处理量
rate(tasks_processed_total[5m])
```

### Grafana仪表板

导入预配置的Grafana仪表板：

```bash
kubectl apply -f k8s/monitoring/grafana-dashboard.yaml
```

---

## 🔄 自动扩缩容

### HPA配置

HorizontalPodAutoscaler根据以下指标自动扩缩容：

1. **CPU利用率**: 目标70%
2. **内存利用率**: 目标80%
3. **活跃连接数**: 每Pod平均100个连接

### 扩缩容策略

**扩容** (60秒稳定窗口):
- 最多增加50%副本数
- 或每次最多2个Pod
- 选择最大值

**缩容** (300秒稳定窗口):
- 最多减少10%副本数
- 或每次最多1个Pod
- 选择最小值

### 手动扩缩容

```bash
# 修改副本数
kubectl patch carpaicluster my-carpai-cluster -n carpai-system \
  --type='merge' -p '{"spec":{"replicas":5}}'

# 或直接编辑
kubectl edit carpaicluster my-carpai-cluster -n carpai-system
```

---

## 🧪 测试部署

### 单节点测试

```yaml
apiVersion: carpai.io/v1alpha1
kind: CarpAICluster
metadata:
  name: test-cluster
  namespace: carpai-system
spec:
  replicas: 1
  image: carpai:latest
  tls:
    enabled: false  # 测试环境可禁用TLS
  auth:
    enabled: false
```

### 多节点生产部署

```yaml
apiVersion: carpai.io/v1alpha1
kind: CarpAICluster
metadata:
  name: prod-cluster
  namespace: carpai-system
spec:
  replicas: 5
  image: carpai:v1.0.0
  tls:
    enabled: true
    secretName: carpai-tls-secret
  auth:
    enabled: true
    jwtSecretRef:
      name: carpai-jwt-secret
      key: secret
  resources:
    requests:
      cpu: "1000m"
      memory: "1Gi"
    limits:
      cpu: "4000m"
      memory: "4Gi"
  autoscaling:
    enabled: true
    minReplicas: 5
    maxReplicas: 30
    targetCPUUtilization: 60
    targetMemoryUtilization: 75
  monitoring:
    enabled: true
    prometheusScrape: true
    metricsPort: 9090
```

---

## 🐛 故障排除

### 检查Pod日志

```bash
# 查看所有Pod日志
kubectl logs -n carpai-system -l app.kubernetes.io/name=carpai

# 查看特定Pod
kubectl logs -n carpai-system carpai-0

# 跟随日志
kubectl logs -f -n carpai-system carpai-0
```

### 检查集群状态

```bash
# 查看CR状态
kubectl describe carpaicluster my-carpai-cluster -n carpai-system

# 查看事件
kubectl get events -n carpai-system --sort-by='.lastTimestamp'
```

### TLS问题

```bash
# 检查证书
kubectl get certificate -n carpai-system

# 检查Secret
kubectl get secret carpai-tls-secret -n carpai-system -o yaml
```

### 网络连接测试

```bash
# 进入Pod
kubectl exec -it carpai-0 -n carpai-system -- /bin/bash

# 测试gRPC连接
grpcurl -plaintext carpai-1.carpai-system.svc.cluster.local:9000 distributed.ClusterNodeService/HealthCheck
```

---

## 📈 性能调优

### 资源建议

| 节点规模 | CPU请求 | 内存请求 | CPU限制 | 内存限制 |
|---------|--------|---------|--------|---------|
| 3节点   | 500m   | 512Mi   | 2000m  | 2Gi     |
| 5节点   | 1000m  | 1Gi     | 4000m  | 4Gi     |
| 10节点  | 2000m  | 2Gi     | 8000m  | 8Gi     |

### gRPC调优

```yaml
env:
  - name: GRPC_MAX_CONCURRENT_STREAMS
    value: "100"
  - name: GRPC_KEEPALIVE_TIME_MS
    value: "10000"
  - name: GRPC_KEEPALIVE_TIMEOUT_MS
    value: "5000"
```

---

## 🔮 未来扩展

- [ ] 跨区域部署支持
- [ ] 基于GPU的工作负载调度
- [ ] 自定义指标扩缩容
- [ ] GitOps集成 (ArgoCD/Flux)
- [ ] Service Mesh集成 (Istio/Linkerd)

---

*最后更新: 2026-05-21*
*版本: v1.0.0*
