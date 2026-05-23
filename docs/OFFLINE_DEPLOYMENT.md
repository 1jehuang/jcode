# CarpAI 离线/局域网部署指南

## 概述

CarpAI支持**Docker Compose**和**Kubernetes**双轨部署方案，完全适配**离线/局域网环境**，无需依赖Docker Desktop或外网连接。

### 部署方案对比

| 特性 | Docker Compose | Kubernetes |
|------|---------------|------------|
| **适用场景** | 单机/小规模集群 (1-3节点) | 大规模集群 (3+节点) |
| **复杂度** | 低 | 中 |
| **资源需求** | 单服务器 | 多节点集群 |
| **高可用** | 基础 | 完整 |
| **自动扩缩容** | 手动 | 自动 |
| **离线支持** | ✓ | ✓ |

---

## 方案一：Docker Compose 部署（推荐单机/小团队）

### 前置要求

- Docker Engine 20.10+ (无需Docker Desktop)
- Docker Compose v2.0+
- Linux: Ubuntu 20.04+, CentOS 7+, 或同等发行版
- 最小配置: 8核CPU, 16GB内存, 100GB磁盘

### 快速开始

#### 1. 在线环境部署

```bash
# 克隆仓库
git clone https://your-git-server/CarpAI.git
cd CarpAI

# 启动企业版（包含Redis Cluster + Milvus + Higress）
docker compose --profile enterprise up -d

# 查看日志
docker compose logs -f jcode-server

# 验证服务
curl http://localhost:8081/api/health
```

#### 2. 离线环境部署

**在有网络的机器上导出镜像：**

```bash
# 导出所有必需镜像
chmod +x scripts/export_images.sh
bash scripts/export_images.sh ./offline-images

# 传输到离线机器
scp -r offline-images user@offline-server:/opt/carpai/
```

**在离线机器上导入并部署：**

```bash
# 导入镜像
chmod +x scripts/import_images.sh
bash scripts/import_images.sh /opt/carpai/offline-images

# 部署
cd /opt/carpai
docker compose --profile enterprise up -d
```

### 配置文件说明

#### docker-compose.yml Profiles

```yaml
profiles:
  - dev          # 开发环境（单节点Redis）
  - enterprise   # 企业版（Redis Cluster + Milvus + Higress）
  - cluster      # Redis Cluster专用
  - milvus       # Milvus向量数据库
  - higress      # Higress网关
  - monitoring   # 监控栈（Prometheus + Grafana）
```

#### 环境变量配置

创建`.env`文件：

```bash
# 数据库密码
POSTGRES_PASSWORD=your_secure_password

# JWT密钥（至少32字符）
JWT_SECRET=your_jwt_secret_key_at_least_32_characters_long

# OAuth2配置（可选）
OAUTH_CLIENT_ID=your_client_id
OAUTH_CLIENT_SECRET=your_client_secret
OAUTH_REDIRECT_URI=http://carpai.your-domain.com/oauth/callback
```

### 服务端口

| 服务 | 端口 | 协议 | 说明 |
|------|------|------|------|
| JCode Server REST | 8081 | HTTP | API接口 |
| JCode Server WebSocket | 8080 | WS | 实时通信 |
| JCode Server gRPC | 50051 | gRPC | 内部通信 |
| PostgreSQL | 5432 | TCP | 数据库 |
| Redis Cluster | 6379-6384 | TCP | 缓存集群 |
| Milvus | 19530 | gRPC | 向量检索 |
| Higress Gateway | 80/443 | HTTP/HTTPS | 网关入口 |
| Higress Admin | 8080 | HTTP | 管理API |

---

## 方案二：Kubernetes 部署（推荐大规模集群）

### 前置要求

- Kubernetes 1.24+ 集群
- kubectl 配置完成
- 持久化存储类（StorageClass）
- Ingress Controller（如NGINX）

### 快速开始

#### 1. 在线环境部署

```bash
# 应用基础配置
kubectl apply -k kubernetes/base

# 或使用overlays定制部署
kubectl apply -k kubernetes/overlays/enterprise
```

#### 2. 离线环境部署

**导出Kubernetes镜像：**

```bash
# 导出所有K8s相关镜像
bash scripts/export_images.sh ./k8s-offline-images

# 同时导出JCode应用镜像
docker save jcode:latest -o k8s-offline-images/jcode_latest.tar
```

**在离线K8s集群导入：**

```bash
# 在每个节点导入镜像
for node in master worker1 worker2; do
    scp k8s-offline-images/*.tar $node:/tmp/
    ssh $node "for f in /tmp/*.tar; do docker load -i \$f; done"
done

# 部署应用
kubectl apply -k kubernetes/overlays/enterprise
```

### Kubernetes目录结构

```
kubernetes/
├── base/                    # 基础配置
│   ├── namespace.yaml       # 命名空间
│   ├── postgres.yaml        # PostgreSQL StatefulSet
│   ├── redis-cluster.yaml   # Redis Cluster StatefulSet
│   └── jcode-server.yaml    # JCode Deployment + Service
└── overlays/                # 环境覆盖
    ├── dev/                 # 开发环境
    │   └── kustomization.yaml
    └── enterprise/          # 企业版
        ├── kustomization.yaml
        └── patches/         # 定制化补丁
```

### 使用Kustomize定制

创建`kubernetes/overlays/enterprise/kustomization.yaml`:

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: carpai

resources:
  - ../../base

patches:
  - path: patches/replica-patch.yaml
  - path: patches/resource-limits.yaml

configMapGenerator:
  - name: jcode-config
    literals:
      - LOG_LEVEL=info
      - AUDIT_ENABLED=true
```

---

## 三阶段客户部署方案

### 第一阶段：200人软件公司（3-5城市分布）

**推荐配置：**
- **部署方案**: Docker Compose
- **节点数**: 1-2台服务器（主备）
- **资源配置**:
  - CPU: 16核
  - 内存: 32GB
  - 磁盘: 500GB SSD
  - 网络: 100Mbps专线连接各城市

**部署命令：**
```bash
# 主节点
docker compose --profile enterprise up -d

# 从节点（只读副本）
docker compose --profile dev up -d
```

**跨城市延迟优化：**
- Redis Cluster会话复制
- KV Cache NVMe本地存储
- Higress网关智能路由

---

### 第二阶段：跨校区职业学校/工程集团（25×200人团队）

**推荐配置：**
- **部署方案**: Kubernetes
- **节点数**: 3-5节点集群
- **资源配置**:
  - Master: 4核/8GB × 3
  - Worker: 16核/64GB × 3
  - 存储: 2TB NVMe共享存储

**部署命令：**
```bash
# 初始化K8s集群
kubeadm init --pod-network-cidr=10.244.0.0/16

# 部署CarpAI
kubectl apply -k kubernetes/overlays/enterprise

# 扩展到5个JCode实例
kubectl scale deployment jcode-server --replicas=5
```

**多租户隔离：**
- Namespace per tenant
- Resource Quotas
- Network Policies

---

### 第三阶段：算力中心（2.5万团队）

**推荐配置：**
- **部署方案**: Kubernetes + Helm Charts
- **节点数**: 50+节点集群
- **跨区域部署**: 多可用区/多云

**Helm Chart部署：**
```bash
# 添加Helm仓库
helm repo add carpai https://charts.carpai.io

# 部署企业版
helm install carpai-enterprise carpai/carpai \
  --namespace carpai \
  --create-namespace \
  --set replicaCount=10 \
  --set resources.limits.cpu=8 \
  --set resources.limits.memory=16Gi
```

**高可用架构：**
- Multi-AZ部署
- etcd集群（5节点）
- 负载均衡器（HAProxy/Keepalived）
- 数据备份（Velero）

---

## 故障排查

### Docker Compose常见问题

**问题1：容器无法启动**
```bash
# 查看详细日志
docker compose logs jcode-server

# 检查依赖服务
docker compose ps
docker compose logs postgres
docker compose logs redis-node-1
```

**问题2：Redis Cluster未初始化**
```bash
# 手动初始化
docker exec -it carpai-redis-node-1 redis-cli --cluster create \
  carpai-redis-node-1:6379 carpai-redis-node-2:6379 carpai-redis-node-3:6379 \
  carpai-redis-node-4:6379 carpai-redis-node-5:6379 carpai-redis-node-6:6379 \
  --cluster-replicas 1 --cluster-yes
```

### Kubernetes常见问题

**问题1：Pod处于Pending状态**
```bash
# 检查事件
kubectl describe pod -n carpai jcode-server-xxx

# 检查资源
kubectl top nodes
kubectl get pvc -n carpai
```

**问题2：服务无法访问**
```bash
# 检查Service
kubectl get svc -n carpai

# 测试连通性
kubectl run test --rm -it --image=busybox --restart=Never -- \
  wget -qO- http://jcode-server.carpai.svc.cluster.local:8081/api/health
```

---

## 性能调优

### PostgreSQL优化

```sql
-- 调整pgvector索引
CREATE INDEX ON documents USING ivfflat (embedding vector_l2_ops) WITH (lists = 100);

-- 分析表
ANALYZE documents;
```

### Redis Cluster优化

```bash
# 调整maxmemory
docker exec carpai-redis-node-1 redis-cli CONFIG SET maxmemory 1gb

# 启用持久化
docker exec carpai-redis-node-1 redis-cli CONFIG SET appendonly yes
```

### JCode Server优化

```yaml
# kubernetes/base/jcode-server.yaml
resources:
  requests:
    cpu: "2"
    memory: 4Gi
  limits:
    cpu: "8"
    memory: 16Gi
```

---

## 安全加固

### 1. 启用TLS

```yaml
# docker-compose.yml
higress:
  volumes:
    - ./certs:/etc/higress/certs:ro
  environment:
    - TLS_ENABLED=true
```

### 2. 网络隔离

```yaml
# kubernetes/base/network-policy.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: carpai-network-policy
spec:
  podSelector:
    matchLabels:
      app: jcode-server
  ingress:
    - from:
        - podSelector:
            matchLabels:
              app: higress
  egress:
    - to:
        - podSelector:
            matchLabels:
              app: postgres
```

### 3. 密钥管理

```bash
# 创建Kubernetes Secret
kubectl create secret generic carpai-secrets \
  --from-literal=postgres-password=$(openssl rand -hex 32) \
  --from-literal=jwt-secret=$(openssl rand -hex 32) \
  -n carpai
```

---

## 备份与恢复

### PostgreSQL备份

```bash
# 备份
docker exec carpai-postgres pg_dump -U carpai carpai > backup.sql

# 恢复
docker exec -i carpai-postgres psql -U carpai carpai < backup.sql
```

### Kubernetes备份（Velero）

```bash
# 安装Velero
velero install --provider aws \
  --plugins velero/velero-plugin-for-aws:v1.8.0 \
  --bucket carpai-backups \
  --backup-location-config region=minio,s3ForcePathStyle=true

# 创建备份
velero backup create carpai-full --include-namespaces carpai

# 恢复
velero restore create --from-backup carpai-full
```

---

## 联系支持

如需部署支持，请联系：
- 技术支持邮箱: support@carpai.io
- 文档: https://docs.carpai.io
- GitHub Issues: https://github.com/CarpAI/issues
