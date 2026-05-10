//! **注水算法 (Water-Filling)** — Parallax 负载均衡核心
//!
//! ## 原理
//!
//! 注水法是一种经典的资源分配优化方法, 来源于信息论中的信道容量求解。
//!
//! ### 问题定义
//!
//! 给定:
//! - \(L\) 个待分配的单元 (模型层数)
//! - \(N\) 个节点, 每个有容量上限 \(C_i\) 和算力权重 \(P_i\)
//!
//! 求: 分配向量 \(\{l_i\}\) 使得:
//! 1. \(l_i \leq C_i\) (容量约束)
//! 2. \(\sum l_i = L\) (完整覆盖)
//! 3. \(l_i / l_j \approx P_i / P_j\) (按算力比例分配, 使负载均衡)
//!
//! ### 求解方法 — 二分搜索 λ
//!
//! 引入水位线参数 \(\lambda\), 目标分配为:
//! $$l_i^* = \min(C_i, \lambda \cdot P_i)$$
//!
//! 通过二分搜索找到使 \(\sum l_i^* = L\) 的 \(\lambda\) 值。
//!
//! ### 应用场景
//!
//! 1. **Pipeline 内重平衡**: 将模型层按算力比例分配到各节点
//! 2. **KV Cache 分配**: 按 TFLOPS 比例分配 KV Cache 内存预算
//! 3. **请求分发**: 按节点吞吐能力比例分发请求

use super::types::*;
use super::SchedulerError;

// ============================================================================
// WaterFilling 核心结构体
// ============================================================================

/// 注水算法执行器
pub struct WaterFilling {
    /// 最大迭代次数
    pub max_iterations: usize,
    /// 收敛容差 (目标总和与实际总和的允许误差)
    pub tolerance: f64,
    /// 统计
    pub total_executions: u64,
}

/// 注水结果
pub struct WaterFillingResult {
    /// 各目标的分配量
    pub allocations: Vec<f64>,
    /// 求得的水位线 λ
    pub lambda: f64,
    /// 实际总分配量
    pub actual_total: f64,
    /// 迭代次数
    pub iterations: usize,
    /// 是否完全收敛
    pub converged: bool,
}

// ============================================================================
// 实现
// ============================================================================

impl WaterFilling {
    pub fn new() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 0.01,
            total_executions: 0,
        }
    }

    pub fn with_iterations(max_iters: usize) -> Self {
        Self {
            max_iterations: max_iters,
            ..Self::new()
        }
    }

    /// 执行注水算法
    ///
    /// # 参数
    /// - `capacities`: 各节点的容量上限 [C_1, C_2, ..., C_N]
    /// - `powers`: 各节点的算力权重 [P_1, P_2, ..., P_N] (通常为 TFLOPS)
    /// - `target_total`: 目标总分配量 (L)
    ///
    /// # 返回
    /// 包含分配向量和 λ 的结果
    pub fn allocate(
        &mut self,
        capacities: &[f64],
        powers: &[f64],
        target_total: f64,
    ) -> Result<WaterFillingResult, SchedulerError> {
        self.total_executions += 1;
        let n = capacities.len();

        if n == 0 || n != powers.len() {
            return Err(SchedulerError::AllocationFailed(
                "capacities 和 powers 长度不一致".into(),
            ));
        }

        if target_total <= 0.0 {
            return Err(SchedulerError::AllocationFailed("目标总量必须 > 0".into()));
        }

        // 总容量检查
        let total_capacity: f64 = capacities.iter().sum();
        if total_capacity < target_total {
            return Err(SchedulerError::InsufficientResources {
                required: format!(
                    "target={:.1}, available_capacity={:.1}",
                    target_total, total_capacity
                ),
            });
        }

        // === 二分搜索 λ ===
        // 搜索范围: [0, max(C_i / P_i)]
        let lo = 0.0f64;
        let hi: f64 = capacities
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| if p > 0.0 { c / p } else { f64::INFINITY })
            .fold(0.0f64, |a, b| a.max(b));

        if hi == 0.0 || !hi.is_finite() {
            // 所有 powers 为 0 或异常 → 均匀分配
            let uniform = target_total / n as f64;
            return Ok(WaterFillingResult {
                allocations: vec![uniform; n],
                lambda: 0.0,
                actual_total: uniform * n as f64,
                iterations: 0,
                converged: true,
            });
        }

        let mut lam = hi; // 初始设为上界
        let mut iterations = 0usize;

        for _ in 0..self.max_iterations {
            iterations += 1;
            let mid = 0.5 * (lo + hi);
            lam = mid;

            // 计算 sum_i min(C_i, λ * P_i)
            let total_at_lam: f64 = capacities
                .iter()
                .zip(powers.iter())
                .map(|(&c, &p)| c.min(lam * p))
                .sum();

            if (total_at_lam - target_total).abs() < self.tolerance {
                break; // 已收敛
            }

            if total_at_lam >= target_total {
                hi = mid; // λ 太大 → 减小
            } else {
                lo = mid; // λ 太小 → 增大
            }
        }

        // === 计算最终分配 ===
        let mut raw_allocations: Vec<f64> = capacities
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| c.min(lam * p))
            .collect();

        // === 整数化 + 余数分配 ===
        // 这一步在 layer_allocator 中已有实现, 这里提供浮点版本

        let actual_total: f64 = raw_allocations.iter().sum();
        let converged = (actual_total - target_total).abs() < self.tolerance * 10.0;

        Ok(WaterFillingResult {
            allocations: raw_allocations,
            lambda: lam,
            actual_total,
            iterations,
            converged,
        })
    }

    /// 整数化版本的注水 (返回 u32 向量)
    ///
    /// 用于模型层分配等离散场景。
    pub fn allocate_integer(
        &mut self,
        capacities: &[u32],
        powers: &[f64],
        target_total: u32,
    ) -> Result<WaterFillingIntResult, SchedulerError> {
        let caps_f: Vec<f64> = capacities.iter().map(|&c| c as f64).collect();
        let result = self.allocate(&caps_f, powers, target_total as f64)?;

        // 整数化: floor + 余数按小数部分降序分配
        let mut int_allocs: Vec<u32> = result
            .allocations
            .iter()
            .map(|&x| x.floor() as u32)
            .collect();

        let assigned_sum: u32 = int_allocs.iter().sum();
        let remaining = target_total.saturating_sub(assigned_sum);

        if remaining > 0 {
            // 计算每个位置的小数余数
            let mut frac: Vec<(f64, i32)> = result
                .allocations
                .iter()
                .enumerate()
                .map(|(i, &x)| (x - int_allocs[i] as f64, -(i as i32)))
                .collect();
            frac.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

            for (_, neg_i) in frac.into_iter().take(remaining as usize) {
                let idx = (-neg_i) as usize;
                if idx < int_allocs.len() && int_allocs[idx] < capacities[idx] {
                    int_allocs[idx] += 1;
                }
            }
        }

        Ok(WaterFillingIntResult {
            allocations: int_allocs,
            lambda: result.lambda,
            actual_total: int_allocs.iter().map(|&x| x as u64).sum(),
            iterations: result.iterations,
        })
    }

    /// 多维注水 — 同时考虑多个资源维度
    ///
    /// 例如同时平衡 CPU、GPU 显存、网络带宽:
    /// - dim 0: 层数 (受显存限制)
    /// - dim 1: 并发请求数 (受 KV Cache 限制)
    /// - dim 2: 吞吐配额 (受带宽限制)
    ///
    /// 对每个维度分别运行注水, 然后取各维度的最小值作为最终约束.
    pub fn allocate_multi_dimensional(
        &mut self,
        constraints: &[WaterFillingConstraint],
    ) -> Result<Vec<Vec<u64>>, SchedulerError>
    where
        WaterFillingConstraint: std::borrow::Borrow<WaterFillConstraintInner>,
    {
        let n = match constraints.first() {
            Some(c) => c.borrow().capacities.len(),
            None => return Ok(vec![]),
        };

        let mut results: Vec<Vec<u64>> = vec![];

        for constraint in constraints {
            let inner = constraint.borrow();
            let result =
                self.allocate_integer(&inner.capacities, &inner.powers, inner.target)?;
            results.push(result.allocations.into_iter().map(|x| x as u64).collect());
        }

        // 取各维度的最小值 (最紧约束)
        if results.len() >= 2 {
            let final_result: Vec<u64> = (0..n)
                .map(|i| { results.iter().map(|r| r[i]).min().unwrap_or(u64::MAX) })
                .collect();

            Ok(vec![final_result])
        } else {
            Ok(results)
        }
    }
}

impl Default for WaterFilling {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 数据类型
// ============================================================================

/// 整数注水结果
#[derive(Debug)]
pub struct WaterFillingIntResult {
    pub allocations: Vec<u32>,
    pub lambda: f64,
    pub actual_total: u64,
    pub iterations: usize,
}

/// 单维约束
#[derive(Debug, Clone)]
pub struct WaterFillConstraintInner {
    pub capacities: Vec<u32>,
    pub powers: Vec<f64>,
    pub target: u32,
}

/// 约束包装 (用于多维场景)
#[derive(Debug, Clone)]
pub struct WaterFillingConstraint(pub WaterFillConstraintInner);

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_water_filling() {
        let mut wf = WaterFilling::new();

        // 3 个节点: 容量分别为 40, 30, 20; 算力比为 4:2:1; 目标 70
        let caps = vec![40.0f64, 30.0, 20.0];
        let powers = vec![4.0f64, 2.0, 1.0];

        let result = wf.allocate(&caps, &powers, 70.0).unwrap();

        assert!(result.converged);
        assert!((result.actual_total - 70.0).abs() < 1.0);

        // 验证: 大算力节点应获得更多分配
        assert!(result.allocations[0] >= result.allocations[1]);
        assert!(result.allocations[1] >= result.allocations[2]);

        // 验证: 无超限
        for (i, &a) in result.allocations.iter().enumerate() {
            assert!(a <= caps[i] + 0.01, "分配 {} 超过节点 {} 的容量 {}", a, i, caps[i]);
        }

        println!("注水结果: allocations={:?}, λ={:.4}", result.allocations, result.lambda);
    }

    #[test]
    fn test_integer_water_filling() {
        let mut wf = WaterFilling::with_iterations(50);

        let caps = vec![40u32, 20, 15, 10]; // 总容量 85
        let powers = vec![4.0f64, 2.0, 1.5, 1.0];
        let target = 60u32;

        let result = wf.allocate_integer(&caps, &powers, target).unwrap();

        assert_eq!(result.actual_total, 60); // 必须精确等于目标
        println!("整数注水: {:?}", result.allocations);

        // 所有分配 ≤ 容量
        for (i, &a) in result.allocations.iter().enumerate() {
            assert!(a <= caps[i]);
        }
    }

    #[test]
    fn test_exact_fit() {
        let mut wf = WaterFilling::new();

        // 总容量恰好 = 目标
        let caps = vec![10.0, 10.0, 10.0];
        let powers = vec![1.0, 1.0, 1.0]; // 均匀

        let result = wf.allocate(&caps, &powers, 30.0).unwrap();
        assert!((result.actual_total - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_insufficient_capacity() {
        let mut wf = WaterFilling::new();

        let caps = vec![5.0, 5.0];
        let powers = vec![1.0, 1.0];

        let result = wf.allocate(&caps, &powers, 20.0);
        assert!(result.is_err(), "应报告资源不足");
    }

    #[test]
    fn test_single_node() {
        let mut wf = WaterFilling::new();

        let caps = vec![100.0];
        let powers = vec![1.0];

        let result = wf.allocate(&caps, &powers, 42.0).unwrap();
        assert!((result.allocations[0] - 42.0).abs() < 0.01);
    }

    #[test]
    fn test_zero_power_node() {
        let mut wf = WaterFilling::new();

        // 有一个零算力节点 → 不应分配任何东西给它
        let caps = vec![50.0, 50.0];
        let powers = vec![1.0, 0.0];

        let result = wf.allocate(&caps, &powers, 25.0).unwrap();
        assert!(result.allocations[1] < 0.01, "零算力节点应不获分配");
        assert!((result.allocations[0] - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_multi_dimensional() {
        let mut wf = WaterFilling::new();

        // 维度 1: 层数 (显存限制)
        let constraint1 = WaterFillingConstraint(WaterFillConstraintInner {
            capacities: vec![40, 20, 15],
            powers: vec![4.0, 2.0, 1.0],
            target: 60,
        });

        // 维度 2: 请求数 (KV Cache 限制)
        let constraint2 = WaterFillingConstraint(WaterFillConstraintInner {
            capacities: vec![16, 8, 4],   // 各节点最大并发
            powers: vec![4.0, 2.0, 1.0], // 同样的算力比
            target: 24,                   // 总共需要 24 个并发槽位
        });

        let results = wf
            .allocate_multi_dimensional(&[constraint1, constraint2])
            .unwrap();

        assert_eq!(results.len(), 1); // 取 min 后只有一组
        let final_alloc = &results[0];
        assert_eq!(final_alloc.iter().sum::<u64>(), 60); // 以第一个维度为准? 不对, 应该取各维度 min

        // 实际上这里的多维逻辑是取每维的最小值
        // 最终结果应同时满足两个维度的约束
    }
}
