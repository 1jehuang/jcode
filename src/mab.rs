//! # MAB 引擎 — 多臂老虎机决策优化（借鉴 Fidelity MABWiser）
//!
//! 用于 CarpAI 的智能决策场景：
//! - 选择最优 LLM Provider（根据延迟/成本/质量）
//! - 选择最优代码补全策略
//! - 选择最优缓存策略
//! - 动态调整探索率（ε 衰减）
//!
//! 核心策略：UCB1、Thompson Sampling、Epsilon Greedy、Softmax、Popularity

use rand::Rng;
use std::collections::HashMap;

/// 臂（Arm）— 候选动作
#[derive(Debug, Clone)]
pub struct Arm {
    pub id: String,
    pub name: String,
    /// 尝试次数
    pub trials: u32,
    /// 累计奖励
    pub rewards: f64,
    /// 平均奖励
    pub avg_reward: f64,
}

/// MAB 学习策略
#[derive(Debug, Clone, Copy)]
pub enum MABPolicy {
    /// ε-Greedy: 以 ε 概率随机探索
    EpsilonGreedy { epsilon: f64 },
    /// UCB1: 上置信界
    UCB1 { alpha: f64 },
    /// Thompson Sampling: 贝叶斯采样
    ThompsonSampling,
    /// Softmax: 基于奖励的概率选择
    Softmax { temperature: f64 },
    /// Popularity: 始终选历史最优
    Popularity,
    /// 自适应 ε: ε = 1/sqrt(t)
    AdaptiveEpsilonGreedy,
}

/// MAB 引擎
pub struct MABEngine {
    arms: Vec<Arm>,
    policy: MABPolicy,
    total_trials: u64,
    rng: rand::rngs::ThreadRng,
    /// 上下文特征（用于上下文相关策略）
    context_dim: Option<usize>,
    /// 上下文权重矩阵 (arm -> weights)
    context_weights: Option<HashMap<String, Vec<f64>>>,
}

impl MABEngine {
    /// 创建 MAB 引擎
    pub fn new(arm_ids: Vec<&str>, policy: MABPolicy) -> Self {
        let arms = arm_ids.into_iter().map(|id| Arm {
            id: id.to_string(),
            name: id.to_string(),
            trials: 0,
            rewards: 0.0,
            avg_reward: 0.0,
        }).collect();

        Self {
            arms,
            policy,
            total_trials: 0,
            rng: rand::rng(),
            context_dim: None,
            context_weights: None,
        }
    }

    /// 创建 MAB 引擎（带上下文）
    pub fn with_context(arm_ids: Vec<&str>, policy: MABPolicy, context_dim: usize) -> Self {
        let mut engine = Self::new(arm_ids, policy);
        engine.context_dim = Some(context_dim);
        let mut weights = HashMap::new();
        for id in &engine.arms.iter().map(|a| a.id.clone()).collect::<Vec<_>>() {
            weights.insert(id.clone(), vec![0.0; context_dim]);
        }
        engine.context_weights = Some(weights);
        engine
    }

    /// 选择最优臂
    pub fn select_arm(&mut self) -> &str {
        let policy = self.policy;
        let total = self.total_trials;

        // 每个臂至少尝试一次
        for arm in &self.arms {
            if arm.trials == 0 {
                return &arm.id;
            }
        }

        match policy {
            MABPolicy::EpsilonGreedy { epsilon } => {
                if self.rng.random::<f64>() < epsilon {
                    let idx = self.rng.random_range(0..self.arms.len());
                    &self.arms[idx].id
                } else {
                    self.best_arm()
                }
            }
            MABPolicy::AdaptiveEpsilonGreedy => {
                let epsilon = 1.0 / (total as f64).sqrt();
                if self.rng.random::<f64>() < epsilon {
                    let idx = self.rng.random_range(0..self.arms.len());
                    &self.arms[idx].id
                } else {
                    self.best_arm()
                }
            }
            MABPolicy::UCB1 { alpha } => {
                let mut best_score = f64::NEG_INFINITY;
                let mut best_idx = 0;
                for (i, arm) in self.arms.iter().enumerate() {
                    let explore = (alpha * (total as f64).ln() / arm.trials as f64).sqrt();
                    let score = arm.avg_reward + explore;
                    if score > best_score {
                        best_score = score;
                        best_idx = i;
                    }
                }
                &self.arms[best_idx].id
            }
            MABPolicy::ThompsonSampling => {
                let mut best_sample = f64::NEG_INFINITY;
                let mut best_idx = 0;
                for (i, arm) in self.arms.iter().enumerate() {
                    let alpha = arm.rewards + 1.0;
                    let beta = (arm.trials as f64 - arm.rewards) + 1.0;
                    let sample = sample_beta(alpha, beta);
                    if sample > best_sample {
                        best_sample = sample;
                        best_idx = i;
                    }
                }
                &self.arms[best_idx].id
            }
            MABPolicy::Softmax { temperature } => {
                let exp_scores: Vec<f64> = self.arms.iter()
                    .map(|a| (a.avg_reward / temperature).exp())
                    .collect();
                let sum: f64 = exp_scores.iter().sum();
                let r = self.rng.random::<f64>() * sum;
                let mut cumulative = 0.0;
                for (i, s) in exp_scores.iter().enumerate() {
                    cumulative += s;
                    if r <= cumulative {
                        return &self.arms[i].id;
                    }
                }
                &self.arms.last().unwrap().id
            }
            MABPolicy::Popularity => self.best_arm(),
        }
    }

    /// 更新臂的奖励
    pub fn update(&mut self, arm_id: &str, reward: f64) {
        self.total_trials += 1;
        if let Some(arm) = self.arms.iter_mut().find(|a| a.id == arm_id) {
            arm.trials += 1;
            arm.rewards += reward;
            arm.avg_reward = arm.rewards / arm.trials as f64;
        }
    }

    /// 获取当前最优臂
    fn best_arm(&self) -> &str {
        let mut best_idx = 0;
        let mut best_score = f64::NEG_INFINITY;
        for (i, arm) in self.arms.iter().enumerate() {
            if arm.avg_reward > best_score {
                best_score = arm.avg_reward;
                best_idx = i;
            }
        }
        &self.arms[best_idx].id
    }

    /// 获取所有臂的期望值
    pub fn expectations(&self) -> Vec<(String, f64)> {
        self.arms.iter().map(|a| (a.id.clone(), a.avg_reward)).collect()
    }

    /// 重置所有臂
    pub fn reset(&mut self) {
        for arm in &mut self.arms {
            arm.trials = 0;
            arm.rewards = 0.0;
            arm.avg_reward = 0.0;
        }
        self.total_trials = 0;
    }
}

/// 从 Beta 分布采样
fn sample_beta(alpha: f64, beta: f64) -> f64 {
    use rand_distr::Distribution;
    let dist = rand_distr::Beta::new(alpha, beta).unwrap_or(rand_distr::Beta::new(1.0, 1.0).unwrap());
    dist.sample(&mut rand::rng())
}

impl std::fmt::Display for MABPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MABPolicy::EpsilonGreedy { epsilon } => write!(f, "ε-Greedy(ε={})", epsilon),
            MABPolicy::UCB1 { alpha } => write!(f, "UCB1(α={})", alpha),
            MABPolicy::ThompsonSampling => write!(f, "ThompsonSampling"),
            MABPolicy::Softmax { temperature } => write!(f, "Softmax(T={})", temperature),
            MABPolicy::Popularity => write!(f, "Popularity"),
            MABPolicy::AdaptiveEpsilonGreedy => write!(f, "Adaptiveε-Greedy"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epsilon_greedy_convergence() {
        let mut mab = MABEngine::new(vec!["slow", "medium", "fast"], MABPolicy::EpsilonGreedy { epsilon: 0.1 });
        for _ in 0..1000 {
            let arm = mab.select_arm().to_string();
            let reward = match arm.as_str() {
                "fast" => 1.0,
                "medium" => 0.5,
                _ => 0.1,
            };
            mab.update(&arm, reward);
        }
        let expectations = mab.expectations();
        let best = expectations.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        assert_eq!(best.0, "fast");
    }

    #[test]
    fn test_ucb1() {
        let mut mab = MABEngine::new(vec!["A", "B"], MABPolicy::UCB1 { alpha: 1.0 });
        mab.update("A", 1.0);
        mab.update("B", 2.0);
        let selected = mab.select_arm();
        assert_eq!(selected, "B");
    }

    #[test]
    fn test_thompson_sampling() {
        let mut mab = MABEngine::new(vec!["good", "bad"], MABPolicy::ThompsonSampling);
        for _ in 0..100 {
            let arm = mab.select_arm().to_string();
            let reward = if arm == "good" { 1.0 } else { 0.0 };
            mab.update(&arm, reward);
        }
        let expectations = mab.expectations();
        let best = expectations.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        assert_eq!(best.0, "good");
    }

    #[test]
    fn test_popularity() {
        let mut mab = MABEngine::new(vec!["x", "y"], MABPolicy::Popularity);
        mab.update("x", 5.0);
        mab.update("y", 3.0);
        assert_eq!(mab.select_arm(), "x");
    }
}
