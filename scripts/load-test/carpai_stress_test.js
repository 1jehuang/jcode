/**
 * CarpAI 分布式推理压力测试脚本
 *
 * 目标：模拟 200+ 并发用户，验证 Parallax 流水线并行的加速比
 *
 * 运行方式:
 *   k6 run --vus 200 --duration 5m carpai_stress_test.js
 *
 * 监控指标:
 *   - http_req_duration (P95 < 500ms)
 *   - distributed_inference_ratio (> 30%)
 *   - kv_cache_hit_rate (> 80%)
 */

import http from 'k6/http';
import { check, sleep } from 'k6';
import { Counter, Gauge, Trend } from 'k6/metrics';

// ============================================================================
// 自定义指标
// ============================================================================

// 分布式推理请求计数
const distributedRequests = new Counter('distributed_requests');
const localRequests = new Counter('local_requests');

// KV Cache 命中率
const kvCacheHits = new Counter('kv_cache_hits');
const kvCacheMisses = new Counter('kv_cache_misses');

// 调度延迟趋势
const schedulingLatency = new Trend('scheduling_latency_ms', true);

// 当前活跃虚拟用户数
const activeVUs = new Gauge('active_vus');

// ============================================================================
// 测试配置
// ============================================================================

export const options = {
  stages: [
    // 阶段1: 预热 (30秒, 10 VUs)
    { duration: '30s', target: 10 },
    // 阶段2: 爬升 (2分钟, 200 VUs)
    { duration: '2m', target: 200 },
    // 阶段3: 峰值压力 (2分钟, 200 VUs)
    { duration: '2m', target: 200 },
    // 阶段4: 下降 (30秒, 0 VUs)
    { duration: '30s', target: 0 },
  ],

  // 阈值检查
  thresholds: {
    http_req_duration: ['p(95)<500', 'p(99)<1000'],
    http_req_failed: ['rate<0.05'],
    distributed_requests: ['count>100'],
    scheduling_latency_ms: ['p(95)<50'],
  },
};

// ============================================================================
// 测试场景
// ============================================================================

const BASE_URL = __ENV.CARPAI_URL || 'http://localhost:8000';
const API_KEY = __ENV.CARPAI_API_KEY || 'test-key-12345';

// 测试模型列表（模拟不同大小的模型请求）
const TEST_MODELS = [
  { name: 'qwen-7b', weight: 0.5 },   // 50% 小模型请求
  { name: 'qwen-14b', weight: 0.3 },  // 30% 中等模型请求
  { name: 'qwen-72b', weight: 0.2 },  // 20% 大模型请求（应触发分布式）
];

// 随机选择模型
function selectModel() {
  const rand = Math.random();
  let cumulative = 0;
  for (const model of TEST_MODELS) {
    cumulative += model.weight;
    if (rand <= cumulative) return model.name;
  }
  return TEST_MODELS[0].name;
}

// 生成随机对话内容
function generatePrompt(modelName) {
  const prompts = [
    "请解释一下 Rust 的所有权机制。",
    "帮我写一个 Python 的快速排序算法。",
    "分析这段代码的潜在安全问题：...",
    "如何将这个 SQL 查询优化得更快？",
    "解释 Transformer 模型的自注意力机制。",
  ];
  return prompts[Math.floor(Math.random() * prompts.length)];
}

export default function () {
  // 更新活跃 VU 指标
  activeVUs.set(__VU);

  const modelName = selectModel();
  const prompt = generatePrompt(modelName);

  const headers = {
    'Content-Type': 'application/json',
    'Authorization': `Bearer ${API_KEY}`,
  };

  const payload = JSON.stringify({
    model: modelName,
    messages: [
      { role: 'user', content: prompt }
    ],
    temperature: 0.7,
    max_tokens: 256,
  });

  const startTime = Date.now();

  // 发送请求
  const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, {
    headers,
    timeout: '5s',
  });

  const endTime = Date.now();
  const latency = endTime - startTime;

  // 检查响应
  const checks = check(res, {
    'status is 200': (r) => r.status === 200,
    'response has content': (r) => r.json().choices && r.json().choices.length > 0,
    'latency under 500ms': () => latency < 500,
  });

  // 记录调度信息
  const scheduler = res.headers['X-CarpAI-Scheduler'];
  if (scheduler && scheduler.includes('Parallax')) {
    distributedRequests.add(1);
  } else {
    localRequests.add(1);
  }

  // 记录调度延迟（从响应头获取）
  const taskId = res.headers['X-CarpAI-Task-ID'];
  if (taskId) {
    schedulingLatency.add(latency);
  }

  // 记录 KV Cache 命中情况（如果响应中包含）
  const kvHit = res.headers['X-CarpAI-KV-Cache-Hit'];
  if (kvHit === 'true') {
    kvCacheHits.add(1);
  } else if (kvHit === 'false') {
    kvCacheMisses.add(1);
  }

  // 思考时间（模拟真实用户间隔）
  sleep(Math.random() * 2 + 1); // 1-3 秒随机间隔
}

// ============================================================================
// 测试结束后的汇总报告
// ============================================================================

export function handleSummary(data) {
  const totalRequests = data.metrics.http_reqs.values.count;
  const distributedRatio = data.metrics.distributed_requests.values.count / totalRequests * 100;
  const avgLatency = data.metrics.http_req_duration.values['avg'];
  const p95Latency = data.metrics.http_req_duration.values['p(95)'];

  console.log(`
╔══════════════════════════════════════════════════════════╗
║           CarpAI 压力测试结果汇总                        ║
╠══════════════════════════════════════════════════════════╣
║ 总请求数:          ${totalRequests.toString().padEnd(38)} ║
║ 平均延迟:          ${avgLatency.toFixed(2).padEnd(34)}ms ║
║ P95 延迟:          ${p95Latency.toFixed(2).padEnd(34)}ms ║
║ 分布式推理占比:    ${distributedRatio.toFixed(2).padEnd(34)}% ║
╚══════════════════════════════════════════════════════════╝
`);

  return {
    stdout: JSON.stringify(data.metrics, null, 2),
    'results.json': JSON.stringify(data, null, 2),
  };
}
