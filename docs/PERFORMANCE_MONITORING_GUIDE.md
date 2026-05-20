# CarpAI Completion Performance Monitoring Guide

## 概述

本文档说明如何收集和监控 CarpAI 补全引擎的真实性能数据，使用 OpenTelemetry 兼容的指标系统。

## ✅ 已完成的监控基础设施

1. **`crates/jcode-completion/src/metrics.rs`** - 完整的指标收集器
2. **集成到 `CompletionEngine::complete()`** - 自动记录所有关键指标
3. **Prometheus 格式输出** - 可直接对接监控系统
4. **Server 启动时激活 LSP Bridge** - `src/cli/dispatch.rs` 已修改

---

## 📊 可用指标

### Counter 指标（累计计数）

| 指标名称 | 类型 | 说明 |
|----------|------|------|
| `jcode_completion_requests_total` | counter | 总补全请求数 |
| `jcode_completion_cache_hits_total` | counter | 预取缓存命中数 |
| `jcode_completion_cache_misses_total` | counter | 预取缓存未命中数 |
| `jcode_completion_acceptances_total` | counter | 用户接受的补全数 |
| `jcode_completion_rejections_total` | counter | 用户拒绝的补全数 |
| `jcode_completion_prefetch_requests_total` | counter | 后台预取请求数 |
| `jcode_completion_errors_total` | counter | 补全错误数 |

### Gauge 指标（瞬时值）

| 指标名称 | 类型 | 说明 |
|----------|------|------|
| `jcode_completion_cache_size` | gauge | 当前缓存大小 |
| `jcode_completion_learned_patterns` | gauge | 已学习的行为模式数 |
| `jcode_completion_latency_ms_avg` | gauge | 平均延迟（毫秒） |
| `jcode_completion_latency_ms_p95` | gauge | P95 延迟（毫秒） |
| `jcode_completion_cache_hit_rate` | gauge | 缓存命中率 (0-1) |
| `jcode_completion_acceptance_rate` | gauge | 接受率 (0-1) |

---

## 🔧 使用方法

### 1. 编程方式访问指标

```rust
use jcode_completion::metrics::get_metrics;

// 获取全局指标实例
let metrics = get_metrics();

// 读取指标值
let total_requests = metrics.total_requests.load(Ordering::Relaxed);
let cache_hit_rate = metrics.get_cache_hit_rate();
let avg_latency = metrics.get_avg_latency_ms();
let p95_latency = metrics.get_p95_latency_ms();

println!("Total requests: {}", total_requests);
println!("Cache hit rate: {:.1}%", cache_hit_rate * 100.0);
println!("Avg latency: {:.0}ms", avg_latency);
println!("P95 latency: {:.0}ms", p95_latency);
```

### 2. 导出 Prometheus 格式

```rust
use jcode_completion::metrics::get_metrics;

let metrics = get_metrics();
let prometheus_output = metrics.generate_prometheus_metrics();

println!("{}", prometheus_output);
```

**输出示例**:
```prometheus
# HELP jcode_completion_requests_total Total completion requests
# TYPE jcode_completion_requests_total counter
jcode_completion_requests_total 1523

# HELP jcode_completion_cache_hits_total Cache hits
# TYPE jcode_completion_cache_hits_total counter
jcode_completion_cache_hits_total 912

# HELP jcode_completion_cache_hit_rate Cache hit rate
# TYPE jcode_completion_cache_hit_rate gauge
jcode_completion_cache_hit_rate 0.5987

# HELP jcode_completion_latency_ms_avg Average completion latency
# TYPE jcode_completion_latency_ms_avg gauge
jcode_completion_latency_ms_avg 78.42
```

### 3. 在 Server 中暴露 /metrics 端点

**文件**: `src/dashboard/routes.rs`

添加路由：

```rust
use jcode_completion::metrics::get_metrics;

pub async fn api_completion_metrics() -> Response {
    let metrics = get_metrics();
    let output = metrics.generate_prometheus_metrics();
    Response::builder()
        .status(200)
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(output.into())
        .unwrap()
}

// In router setup:
.route("/api/completion-metrics", get(api_completion_metrics))
```

---

## 📈 监控面板配置

### Grafana Dashboard JSON

导入以下面板到 Grafana：

```json
{
  "dashboard": {
    "title": "CarpAI Completion Metrics",
    "panels": [
      {
        "title": "Request Rate",
        "type": "graph",
        "targets": [
          {
            "expr": "rate(jcode_completion_requests_total[5m])",
            "legendFormat": "Requests/sec"
          }
        ]
      },
      {
        "title": "Cache Hit Rate",
        "type": "gauge",
        "targets": [
          {
            "expr": "jcode_completion_cache_hit_rate",
            "legendFormat": "Hit Rate"
          }
        ],
        "thresholds": [
          { "value": 0, "color": "red" },
          { "value": 0.5, "color": "yellow" },
          { "value": 0.7, "color": "green" }
        ]
      },
      {
        "title": "Latency (P95)",
        "type": "graph",
        "targets": [
          {
            "expr": "jcode_completion_latency_ms_p95",
            "legendFormat": "P95 Latency"
          }
        ]
      },
      {
        "title": "Acceptance Rate",
        "type": "stat",
        "targets": [
          {
            "expr": "jcode_completion_acceptance_rate",
            "legendFormat": "Acceptance Rate"
          }
        ]
      }
    ]
  }
}
```

---

## 🧪 性能测试

### 基准测试脚本

```bash
#!/bin/bash
# scripts/benchmark_completion.sh

echo "Starting completion benchmark..."

# Warm up cache
for i in {1..10}; do
    curl -s http://localhost:8080/api/completion \
        -d '{"file":"src/main.rs","line":10,"column":5}' > /dev/null
done

# Measure performance
start_time=$(date +%s%N)
for i in {1..100}; do
    curl -s http://localhost:8080/api/completion \
        -d '{"file":"src/main.rs","line":'$((RANDOM % 100 + 1))',"column":5}' > /dev/null
done
end_time=$(date +%s%N)

elapsed=$(( (end_time - start_time) / 1000000 ))
echo "Completed 100 requests in ${elapsed}ms"
echo "Average: $(echo "scale=2; $elapsed / 100" | bc)ms per request"

# Get metrics
curl -s http://localhost:8080/api/completion-metrics | grep "cache_hit_rate"
```

### 负载测试

使用 `wrk` 或 `hey` 进行并发测试：

```bash
# Install hey
cargo install hey

# Run load test
hey -n 1000 -c 10 -m POST \
    -d '{"file":"test.rs","line":10,"column":5}' \
    http://localhost:8080/api/completion

# Check metrics after load
curl http://localhost:8080/api/completion-metrics
```

---

## 🚨 告警规则

### Prometheus Alert Rules

```yaml
groups:
  - name: completion_alerts
    rules:
      # Low cache hit rate
      - alert: LowCacheHitRate
        expr: jcode_completion_cache_hit_rate < 0.3
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Completion cache hit rate is low"
          description: "Cache hit rate is {{ $value }}, expected > 0.5"

      # High latency
      - alert: HighCompletionLatency
        expr: jcode_completion_latency_ms_p95 > 200
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "Completion latency is high"
          description: "P95 latency is {{ $value }}ms, threshold is 200ms"

      # Low acceptance rate
      - alert: LowAcceptanceRate
        expr: jcode_completion_acceptance_rate < 0.5
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Users are rejecting completions"
          description: "Acceptance rate is {{ $value }}, expected > 0.6"

      # High error rate
      - alert: HighErrorRate
        expr: rate(jcode_completion_errors_total[5m]) > 0.1
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "High completion error rate"
          description: "Error rate is {{ $value }}/sec"
```

---

## 📊 数据收集和分析

### 1. 定期快照

```bash
#!/bin/bash
# scripts/collect_metrics.sh

METRICS_DIR="/var/log/jcode/metrics"
mkdir -p "$METRICS_DIR"

timestamp=$(date +%Y%m%d_%H%M%S)
curl -s http://localhost:8080/api/completion-metrics > \
    "$METRICS_DIR/completion_${timestamp}.prom"

echo "Metrics collected at $timestamp"
```

### 2. 趋势分析

```python
# scripts/analyze_trends.py
import pandas as pd
import matplotlib.pyplot as plt
import glob

# Load metrics files
files = sorted(glob.glob('/var/log/jcode/metrics/*.prom'))
data = []

for f in files:
    with open(f) as fp:
        for line in fp:
            if 'cache_hit_rate' in line and not line.startswith('#'):
                value = float(line.split()[1])
                timestamp = os.path.getmtime(f)
                data.append({'time': timestamp, 'hit_rate': value})

df = pd.DataFrame(data)
plt.plot(df['time'], df['hit_rate'])
plt.xlabel('Time')
plt.ylabel('Cache Hit Rate')
plt.title('Completion Cache Performance Trend')
plt.savefig('/tmp/cache_hit_rate_trend.png')
```

### 3. A/B 测试对比

```rust
// Compare metrics between two configurations
use jcode_completion::metrics::get_metrics;

fn run_ab_test() {
    // Configuration A: Standard prefetch
    let metrics_a = get_metrics();
    metrics_a.reset();
    run_test_with_config(Config::default());
    let hit_rate_a = metrics_a.get_cache_hit_rate();
    let latency_a = metrics_a.get_avg_latency_ms();

    // Configuration B: Aggressive prefetch
    metrics_a.reset();
    run_test_with_config(Config { aggressive_prefetch: true });
    let hit_rate_b = metrics_a.get_cache_hit_rate();
    let latency_b = metrics_a.get_avg_latency_ms();

    println!("Config A: hit_rate={:.1}%, latency={:.0}ms",
             hit_rate_a * 100.0, latency_a);
    println!("Config B: hit_rate={:.1}%, latency={:.0}ms",
             hit_rate_b * 100.0, latency_b);
}
```

---

## 🔍 故障排查

### 问题 1: 指标不更新

**症状**: 指标值始终为 0

**检查步骤**:
```bash
# 1. 确认 Server 正在运行
ps aux | grep jcode

# 2. 检查日志
tail -f ~/.jcode/logs/jcode-*.log | grep "completion"

# 3. 验证端点可访问
curl http://localhost:8080/api/completion-metrics
```

**解决方案**:
- 确保 `enable_lsp_globally()` 被调用
- 检查 `CompletionEngine` 是否正确初始化

### 问题 2: 缓存命中率异常低

**症状**: `cache_hit_rate < 0.2`

**可能原因**:
1. 用户在多个文件间频繁切换
2. 编辑模式高度随机
3. 预取阈值过高

**诊断**:
```rust
let metrics = get_metrics();
println!("Hits: {}", metrics.cache_hits.load(Ordering::Relaxed));
println!("Misses: {}", metrics.cache_misses.load(Ordering::Relaxed));
println!("Prefetch requests: {}", metrics.prefetch_requests.load(Ordering::Relaxed));
```

**优化建议**:
- 降低 `PREFETCH_CONFIDENCE_THRESHOLD`（在 `streaming_prefetch.rs` 中）
- 增加缓存大小（`MAX_PRELOAD_CACHE_SIZE`）
- 调整 debounce 间隔

### 问题 3: 延迟突然升高

**症状**: `p95_latency > 300ms`

**检查**:
```bash
# Check system resources
top -p $(pgrep jcode)

# Check LSP status
curl http://localhost:8080/api/lsp-status
```

**可能原因**:
- LSP server 重启
- 网络延迟
- CPU 资源竞争

---

## 📝 最佳实践

### 1. 监控频率

- **开发环境**: 每 5 秒采集一次
- **生产环境**: 每 15-30 秒采集一次
- **告警评估**: 每 1-5 分钟评估一次

### 2. 基线建立

运行至少 1 周收集基线数据：
```bash
# Daily metrics summary
curl http://localhost:8080/api/completion-metrics | \
    grep -E "(hit_rate|latency)" >> /var/log/jcode/daily_metrics.log
```

### 3. 容量规划

根据指标决定资源分配：
- **Cache size > 150**: 考虑增加内存
- **Error rate > 5%**: 检查 LSP 稳定性
- **Latency > 150ms**: 优化 LLM provider 连接

---

## 🎯 下一步行动

1. **立即**: 启动 Server 并验证指标端点
   ```bash
   cargo run --release -- serve
   curl http://localhost:8080/api/completion-metrics
   ```

2. **本周内**: 配置 Grafana 面板
   - 导入 dashboard JSON
   - 设置数据源
   - 配置告警

3. **本月内**: 建立性能回归测试
   - 自动化基准测试
   - CI/CD 集成
   - 性能预算设定

---

*文档版本: v1.0*
*创建日期: 2026-05-21*
*状态: ✅ 监控基础设施已完成*
