# CarpAI 分布式部署开发任务清单

## 项目背景

**目标架构**: 3台主节点（64G内存/4G显存）+ 15台网吧机器（动态接入）
**总资源**: 18节点，总计72GB显存，~1.15TB内存
**部署模型**: DeepSeek V4-flash, Qwen3.6-35B, GLM 5.1

---

## 当前实现程度评估

### ✅ 已实现的核心功能 (完成度 ~75%)

#### 1. 集群管理基础框架
- [x] `ClusterManager` - 节点注册/注销 ([src/distributed/cluster.rs](src/distributed/cluster.rs:27-51))
- [x] Raft风格领导选举 ([src/distributed/election.rs](src/distributed/election.rs))
- [x] 健康检查与心跳机制 ([src/distributed/service.rs](src/distributed/service.rs:254-320))
- [x] 5种负载均衡策略 ([src/distributed/load_balancer.rs](src/distributed/load_balancer.rs))

#### 2. 分布式推理引擎
- [x] 流水线并行架构 (`crates/jcode-distributed-inference/`)
- [x] KV Cache传输 (gRPC Stream)
- [x] LayerExecutor (Candle后端)
- [x] Worker gRPC服务实现

#### 3. 统一调度器
- [x] Roofline性能模型 ([resource_node.rs:321](crates/jcode-unified-scheduler/src/resource_node.rs:321))
- [x] 注水负载均衡算法 ([layer_allocator.rs:696](crates/jcode-unified-scheduler/src/layer_allocator.rs:696))
- [x] Greedy/DP分配策略
- [x] 动态节点加入接口 ([layer_allocator.rs:200](crates/jcode-unified-scheduler/src/layer_allocator.rs:200))

#### 4. Kubernetes部署
- [x] Deployment + Service + Ingress
- [x] HorizontalPodAutoscaler (HPA)
- [x] CRD自定义资源定义
- [x] Prometheus监控集成

---

### ⚠️ 待完善的关键功能

#### P0 - 核心功能阻塞项（必须完成）

##### 1. 完善节点移除后的层重新分配逻辑
**问题**: [service.rs:316](src/distributed/service.rs:316) 存在TODO标记
```rust
for node in unhealthy {
    warn!("Node {} is unhealthy", node.id);
    // TODO: Implement node removal or alerting
}
```

**需要实现**:
- [ ] 检测到离线节点后触发层重新分配
- [ ] 从`LayerAllocator`中移除故障节点的层分配
- [ ] 将未分配的层重新分配给健康节点
- [ ] 更新Pipeline拓扑结构
- [ ] 通知Coordinator重新路由请求

**涉及文件**:
- `src/distributed/service.rs` - health_check_loop
- `crates/jcode-unified-scheduler/src/layer_allocator.rs` - 新增`remove_node_and_rebalance()`
- `crates/jcode-distributed-inference/src/coordinator_client.rs` - 更新路由表

**验收标准**:
- 模拟节点离线后，系统在30秒内完成重平衡
- 正在进行的推理请求不中断（优雅降级）
- 重平衡后Pipeline仍然完整覆盖所有层

---

##### 2. 实现健康检查中的自动故障转移机制
**问题**: 当前仅检测不健康节点，无自动恢复

**需要实现**:
- [ ] 故障节点分级处理（警告 -> 隔离 -> 移除）
- [ ] 基于负载的自动故障转移决策
- [ ] 故障恢复后的节点重新加入流程
- [ ] 告警通知机制（日志/邮件/Webhook）

**设计思路**:
```rust
enum NodeHealthState {
    Healthy,      // 正常
    Warning,      // 连续2次心跳超时
    Critical,     // 连续5次心跳超时
    Offline,      // 超过timeout阈值
}

struct FaultToleranceManager {
    failure_history: HashMap<NodeId, Vec<FailureEvent>>,
    auto_recovery: bool,
    max_retry_count: u32,
}
```

**涉及文件**:
- `src/distributed/service.rs` - 增强health_check_loop
- 新建 `src/distributed/fault_tolerance.rs`

**验收标准**:
- 支持配置化的故障阈值
- 故障转移决策基于多维度指标（心跳、负载、错误率）
- 支持手动/自动恢复模式

---

##### 3. 添加大规模集群集成测试（18节点场景）
**问题**: 当前测试仅覆盖小规模场景（3节点）

**需要实现**:
- [ ] 18节点集群启动测试
- [ ] 节点频繁加入/退出压力测试
- [ ] 网络分区模拟测试
- [ ] 高并发推理请求测试
- [ ] 故障注入测试（Chaos Engineering）

**测试场景**:
```rust
#[tokio::test]
async fn test_18_node_cluster_stability() {
    // 1. 启动18节点集群
    // 2. 分配Qwen3.6-35B模型（40层）
    // 3. 模拟3个节点同时离线
    // 4. 验证系统自动重平衡
    // 5. 模拟节点重新加入
    // 6. 验证Pipeline完整性
}

#[tokio::test]
async fn test_internet_cafe_dynamic_join() {
    // 模拟网吧机器批量上线/下线
    // 验证dynamic_join的性能和正确性
}
```

**涉及文件**:
- `src/distributed/integration_tests.rs` - 新增大规模测试
- 新建 `tests/large_scale_cluster/` 目录

**验收标准**:
- 18节点集群启动时间 < 10秒
- 单节点加入/退出处理时间 < 2秒
- 故障恢复时间 < 30秒
- 测试覆盖率 > 80%

---

#### P1 - 高优先级优化项

##### 4. 优化KV Cache传输性能（压缩+批量）
**问题**: 当前KV Cache传输未压缩，带宽占用高

**需要实现**:
- [ ] FP16 -> INT8量化压缩（可选）
- [ ] LZ4/Zstd压缩算法
- [ ] 批量传输（减少gRPC调用次数）
- [ ] 零拷贝序列化（使用Arrow/FlatBuffers）

**性能目标**:
- 压缩比 > 2x（无损）或 > 4x（有损）
- 传输延迟降低 > 50%
- CPU开销增加 < 10%

**涉及文件**:
- `crates/jcode-distributed-inference/src/kv_cache_manager.rs`
- `crates/jcode-distributed-inference/proto/inference.proto`

**技术方案**:
```protobuf
message KVCacheChunk {
    bytes compressed_data = 1;  // 新增
    string compression_algo = 2; // "lz4", "zstd", "none"
    bool quantized = 3;          // 是否INT8量化
    // ... 其他字段
}
```

---

##### 5. 实现模型热切换和优雅下线机制
**问题**: 当前`stop()`直接移除实例，无优雅下线

**需要实现**:
- [ ] 等待活跃请求完成（graceful shutdown）
- [ ] 新请求拒绝但返回重试提示
- [ ] 状态快照保存（支持快速恢复）
- [ ] 蓝绿部署支持（零停机切换）

**设计思路**:
```rust
pub async fn graceful_stop(&self, model_name: &str, timeout_secs: u64) -> Result<()> {
    // 1. 标记为 draining 状态（不再接受新请求）
    // 2. 等待活跃请求完成或超时
    // 3. 保存KV Cache快照
    // 4. 停止实例
    // 5. 清理资源
}

pub async fn hot_swap(&self, old_model: &str, new_model: &str) -> Result<()> {
    // 1. 启动新模型实例
    // 2. 预热（加载权重、初始化KV Cache）
    // 3. 原子切换路由
    // 4. 优雅关闭旧模型
}
```

**涉及文件**:
- `crates/jcode-cpu-inference/src/engine.rs`
- `crates/jcode-distributed-inference/src/coordinator.rs`

---

##### 6. 增强网络分区容忍性和状态同步
**问题**: 当前仅有基础Quorum检查，无完整分区处理

**需要实现**:
- [ ] 网络分区检测（RTT突增、丢包率）
- [ ] Split-brain预防（严格Quorum写入）
- [ ] 分区恢复后的状态合并（Last-Writer-Wins或向量时钟）
- [ ] 跨区域部署支持（多可用区）

**技术方案**:
```rust
struct NetworkPartitionDetector {
    rtt_history: HashMap<NodeId, Vec<(Timestamp, f64)>>,
    packet_loss_rate: HashMap<NodeId, f64>,
    partition_threshold_ms: f64,  // RTT > 100ms 视为分区
}

enum ConsistencyLevel {
    Strong,   // 需要Quorum确认
    Eventual, // 异步复制
}
```

**涉及文件**:
- `src/distributed/election.rs` - 增强选举逻辑
- 新建 `src/distributed/partition_detector.rs`

---

#### P2 - 中优先级功能增强

##### 7. 添加NUMA/GPU拓扑感知调度
**问题**: 当前调度不考虑硬件拓扑

**需要实现**:
- [ ] NUMA节点感知（避免跨NUMA访问）
- [ ] NVLink/P2P拓扑利用（优先同GPU通信）
- [ ] PCIe带宽考虑
- [ ] 缓存亲和性

**数据结构**:
```rust
struct HardwareTopology {
    numa_nodes: Vec<NumaNode>,
    gpu_topology: GpuTopologyGraph,
    pcie_bandwidth: HashMap<(NodeId, NodeId), f64>,
}

struct NumaNode {
    id: u32,
    cpu_cores: Vec<u32>,
    memory_gb: f64,
    attached_gpus: Vec<NodeId>,
}
```

---

##### 8. 实现细粒度资源配置
**问题**: 当前仅支持基础显存配置

**需要实现**:
- [ ] 显存碎片管理
- [ ] 内存带宽预留
- [ ] GPU计算单元配额
- [ ] 存储I/O带宽限制

---

##### 9. 完善动态节点加入的完整流程
**问题**: `dynamic_join()`已实现但未完全集成

**需要实现**:
- [ ] 节点加入时的能力通告（支持的模型、显存大小）
- [ ] 增量重平衡（不影响现有Pipeline）
- [ ] 节点预热（预加载模型权重）
- [ ] 灰度上线（先分配少量流量验证）

**当前代码位置**: [layer_allocator.rs:200](crates/jcode-unified-scheduler/src/layer_allocator.rs:200)

---

#### P3 - 低优先级扩展功能

##### 10. 添加跨区域部署支持
**需要实现**:
- [ ] 多区域集群联邦
- [ ] 跨区域数据复制
- [ ] 智能路由（就近访问）
- [ ] 灾难恢复

---

### 📊 实施路线图

#### Phase 1: 稳定性加固（2周）
1. 完成P0三项核心功能
2. 编写18节点集成测试
3. 修复所有TODO标记

#### Phase 2: 性能优化（1周）
4. KV Cache传输优化
5. 模型热切换实现
6. 网络分区增强

#### Phase 3: 功能完善（1周）
7. NUMA拓扑感知
8. 细粒度资源配置
9. 动态节点流程完善

#### Phase 4: 生产验证（1周）
- 在3台主节点上部署测试
- 逐步接入网吧机器验证
- 压力测试和调优

---

## 具体技术建议

### 针对您的硬件配置

#### 显存预算分析
```
单节点: 4GB显存
3台主节点: 12GB
15台网吧机器: 60GB
总计: 72GB

模型需求估算（INT4量化）:
- Qwen3.6-35B: ~20GB (权重) + ~10GB (KV Cache) = 30GB
- DeepSeek V4-flash: 需确认具体版本，预估 40-60GB
- GLM 5.1: 需确认版本

建议:
1. 优先部署 Qwen3.6-35B (可行)
2. DeepSeek 选择较小版本或使用更高量化 (INT2/INT3)
3. GLM 5.1 如果是小版本可部署
```

#### 流水线切分建议
```
Qwen3.6-35B (40层):
- 方案A (3节点): 每节点13-14层 -> 显存不足 ❌
- 方案B (18节点): 每节点2-3层 -> 可行 ✅

推荐配置:
- 3台主节点: Coordinator + 首尾节点 (Embedding/LM Head)
- 15台网吧机器: Worker节点 (每节点2-3层)
```

#### 网络要求
```
- 局域网延迟: < 1ms (千兆以太网)
- KV Cache传输带宽: ~100MB/s per request
- 建议: 万兆交换机或至少千兆骨干网
```

---

## 下一步行动

### 立即开始（本周）
1. **阅读并理解现有代码**:
   - `src/distributed/service.rs`
   - `crates/jcode-unified-scheduler/src/layer_allocator.rs`
   - `crates/jcode-distributed-inference/src/worker.rs`

2. **搭建开发环境**:
   ```bash
   ./scripts/dev_setup.sh
   cargo build --release
   ```

3. **运行现有测试**:
   ```bash
   cargo test distributed
   cargo test unified_scheduler
   ```

4. **开始实现P0任务1**: 节点移除后的层重新分配

### 预期交付物
- Week 1-2: P0功能完成 + 集成测试框架
- Week 3: P1优化完成
- Week 4: 小规模部署验证（3节点）
- Week 5-6: 全量部署（18节点）+ 压力测试

---

## 风险与缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|----------|
| 网吧机器不稳定 | 高 | 高 | 实现快速故障转移，设置冗余节点 |
| 网络带宽瓶颈 | 高 | 中 | KV Cache压缩，批量传输 |
| 显存不足 | 高 | 中 | 使用INT4量化，调整模型选择 |
| 分布式一致性bug | 高 | 低 | 充分测试，使用成熟的Raft实现 |
| 开发周期超预期 | 中 | 中 | 优先P0功能，P1/P2可延后 |

---

## 参考资料

- Parallax论文: Pipeline Parallelism for LLM Inference
- Raft论文: In Search of an Understandable Consensus Algorithm
- Roofline模型: A Performance Model for CPUs and GPUs

---

**最后更新**: 2026-05-21
**负责人**: 开发团队
**审核人**: 架构师
