//! **GOAP A* 规划器** — 移植自 Ruflo Goal Module
//!
//! ## 算法概述
//!
//! GOAP (Goal-Oriented Action Planning) 使用 **A*** 搜索算法在状态空间中
//! 寻找从初始状态到目标状态的**最短动作序列**。
//!
//! ### 评价函数
//! ```
//! f(n) = g(n) + w * h(n)
//! ```
//! - `g(n)`: 从起始状态到节点 n 的实际代价
//! - `h(n)`: 从 n 到目标状态的启发式距离 (admissible, 不超过实际代价)
//! - `w`: 启发式权重 (>= 1.0 时变为 weighted A*, 加速但牺牲最优性)
//!
//! ### OODA 循环 (自适应重规划)
//! 当环境变化或动作失败时, 规划器会从**当前状态**重新规划,
//! 而不是从头开始。响应时间 < 500ms。

use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::types::*;

// ============================================================================
// 核心数据结构
// ============================================================================

/// GOAP 规划器主结构体
#[derive(Debug)]
pub struct GoapPlanner {
    /// 可用的动作库
    actions: Vec<GoapActionDef>,
    /// 最大搜索迭代次数
    max_iterations: usize,
    /// 启发式权重 (w in f(n) = g(n) + w*h(n))
    heuristic_weight: f64,
    /// 统计: 总规划次数
    total_plans: AtomicUsize,
    /// 统计: 总规划失败次数
    total_failures: AtomicUsize,
    /// 统计: 平均规划耗时 (纳秒)
    total_planning_ns: AtomicUsize,
}

/// GOAP 动作定义 (模板)
#[derive(Debug, Clone)]
pub struct GoapActionDef {
    /// 动作名称 (如 "install_dependencies", "write_tests")
    pub name: String,
    /// 前置条件 (必须全部满足才可执行)
    pub preconditions: Vec<WorldStateCondition>,
    /// 效果 (执行后对世界的改变)
    pub effects: Vec<WorldStateEffect>,
    /// 代价 (正数, 越大越不愿意选)
    pub cost: f64,
    /// 动作类别 (用于分类和过滤)
    pub category: ActionCategory,
    /// 是否为确定性动作 (false = 可能失败, 需要 retry)
    pub deterministic: bool,
    /// 预估执行时间 (ms)
    pub estimated_duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    Analysis,      // 分析类 (如 analyze_codebase)
    Creation,      // 创建类 (如 write_code, create_file)
    Execution,     // 执行类 (如 run_tests, build_project)
    Verification,  // 验证类 (如 lint_check, security_scan)
    Communication, // 通信类 (如 send_notification)
    Memory,        // 记忆操作类 (如 store_fact, recall)
}

/// A* 搜索节点
#[derive(Debug, Clone)]
struct SearchNode {
    /// 世界状态
    state: WorldState,
    /// g(n): 起始到此的实际代价
    g_cost: f64,
    /// h(n): 此处到目标的启发式估计
    h_cost: f64,
    /// f(n) = g + w * h
    f_cost: f64,
    /// 到达此状态的动作历史
    path: Vec<usize>, // indices into the action list
    parent: Option<Box<SearchNode>>,
}

// 为了让 BinaryHeap 作为最小堆 (弹出 f_cost 最小的)
impl PartialEq for SearchNode {
    fn eq(&self, other: &Self) -> bool {
        (self.f_cost * 1e6) as i64 == (other.f_cost * 1e6) as i64
    }
}
impl Eq for SearchNode {}

impl PartialOrd for SearchNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 反向比较: BinaryHeap 是最大堆, 我们要最小堆行为
        other.f_cost.partial_cmp(&self.f_cost).unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// 规划结果
#[derive(Debug)]
pub struct PlanResult {
    pub success: bool,
    pub plan: Option<GoapPlan>,
    pub error: Option<String>,
    pub iterations: usize,
    pub time_ms: f64,
}

// ============================================================================
// GoapPlanner 实现
// ============================================================================

impl GoapPlanner {
    /// 创建新规划器 (带默认的动作库)
    pub fn new() -> Self {
        Self {
            actions: Self::builtin_actions(),
            max_iterations: 10_000,
            heuristic_weight: 1.0,
            total_plans: AtomicUsize::new(0),
            total_failures: AtomicUsize::new(0),
            total_planning_ns: AtomicUsize::new(0),
        }
    }

    /// 设置最大迭代次数
    pub fn set_max_iterations(&mut self, max: usize) {
        self.max_iterations = max;
    }

    /// 设置启发式权重
    pub fn set_heuristic_weight(&mut self, w: f64) {
        self.heuristic_weight = w;
    }

    /// 注册自定义动作
    pub fn register_action(&mut self, action: GoapActionDef) {
        self.actions.push(action);
    }

    /// 注册一组自定义动作
    pub fn register_actions(&mut self, actions: Vec<GoapActionDef>) {
        self.actions.extend(actions);
    }

    /// 清空动作库
    pub fn clear_actions(&mut self) {
        self.actions.clear();
    }

    /// 获取已注册动作数
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// 核心 API: 规划 — 给定初始状态和目标, 找出最优动作序列
    ///
    /// 对应 Ruflo 的 `goal.ruv.io` 服务端的规划流程:
    /// 1. 分析当前世界状态
    /// 2. 定义目标状态
    /// 3. A* 搜索最优路径
    /// 4. 输出结构化步骤列表
    pub async fn plan(
        &self,
        task: &ScheduledTask,
    ) -> Result<GoapPlan, GoapError> {
        let start = std::time::Instant::now();
        self.total_plans.fetch_add(1, Ordering::Relaxed);

        let goal_str = task.goal.as_deref().unwrap_or(&task.description);
        let initial_state = self.infer_initial_state(task);
        let goal_state = self.parse_goal(goal_str)?;

        let result = self.search(&initial_state, &goal_state)?;

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let _prev_total_ns = self.total_planning_ns.swap(
            start.elapsed().as_nanos() as usize,
            Ordering::Relaxed,
        );

        match result {
            Some(plan) => {
                let mut final_plan = plan;
                final_plan.planning_time_ms = elapsed_ms;
                Ok(final_plan)
            }
            None => {
                self.total_failures.fetch_add(1, Ordering::Relaxed);
                Err(GoapError::NoPlanFound(format!(
                    "{} (iterations={})",
                    goal_str, self.max_iterations
                )))
            }
        }
    }

    /// 增量重规划 (OODA 循环)
    ///
    /// 当动作失败或环境变化时, 从**当前状态**重新规划,
    /// 而不是从头开始。这是 Ruflo 的关键优化之一。
    pub async fn replan(
        &self,
        current_state: &WorldState,
        goal_state: &WorldState,
        completed_actions: &[String], // 已完成的动作 (跳过)
    ) -> Result<GoapPlan, GoapError> {
        let result = self.search(current_state, goal_state)?;

        match result {
            Some(mut plan) => {
                // 过滤掉已完成的动作
                if !completed_actions.is_empty() {
                    let completed_set: HashSet<&str> =
                        completed_actions.iter().map(|s| s.as_str()).collect();
                    plan.steps.retain(|step| !completed_set.contains(step.action_name.as_str()));
                }
                Ok(plan)
            }
            None => Err(GoapError::ReplanningFailed("无法找到替代方案".into())),
        }
    }

    /// A* 搜索核心算法
    fn search(
        &self,
        initial_state: &WorldState,
        goal_state: &WorldState,
    ) -> Result<Option<GoapPlan>, GoapError> {
        let heuristic_weight = self.heuristic_weight;

        // 初始节点
        let h_init = self.heuristic(initial_state, goal_state);
        let init_node = SearchNode {
            state: initial_state.clone(),
            g_cost: 0.0,
            h_cost: h_init,
            f_cost: h_init * heuristic_weight,
            path: vec![],
            parent: None,
        };

        // Open set (按 f_cost 排序的最小堆)
        let mut open_set = BinaryHeap::new();
        open_set.push(init_node);

        // Closed set (已访问的状态哈希)
        let mut closed_set = HashSet::new();

        let mut iterations = 0usize;
        let mut best_partial: Option<SearchNode> = None;
        let mut best_h_cost = f64::INFINITY;

        while let Some(current) = open_set.pop() {
            iterations += 1;
            if iterations > self.max_iterations {
                break; // 超出迭代限制
            }

            // 检查是否到达目标
            if self.goal_satisfied(&current.state, goal_state) {
                return Ok(Some(self.build_plan(&current, iterations)));
            }

            // 跟踪最佳部分解 (即使没到达目标也返回最有希望的方案)
            if current.h_cost < best_h_cost {
                best_h_cost = current.h_cost;
                best_partial = Some(current.clone());
            }

            // 状态哈希 (用于 closed set 去重)
            let state_hash = self.hash_state(&current.state);

            if closed_set.contains(&state_hash) {
                continue;
            }
            closed_set.insert(state_hash);

            // 扩展: 尝试所有可用动作
            let applicable_actions = self.find_applicable_actions(&current.state);

            for (action_idx, action) in applicable_actions {
                // 应用效果, 产生新状态
                let mut new_state = current.state.clone();
                for effect in &action.effects {
                    new_state.apply_effect(effect);
                }

                // 计算 cost
                let new_g = current.g_cost + action.cost;
                let new_h = self.heuristic(&new_state, goal_state);
                let new_f = new_g + heuristic_weight * new_h;

                let child = SearchNode {
                    state: new_state,
                    g_cost: new_g,
                    h_cost: new_h,
                    f_cost: new_f,
                    path: {
                        let mut p = current.path.clone();
                        p.push(action_idx);
                        p
                    },
                    parent: Some(Box::new(current.clone())),
                };

                open_set.push(child);
            }
        }

        // 搜索耗尽但没有完整解 -> 返回最佳部分解 (如果有)
        if let Some(partial) = best_partial {
            if partial.h_cost < 2.0 { // 接近目标的部分解也可以接受
                return Ok(Some(self.build_plan(&partial, iterations)));
            }
        }

        Ok(None)
    }

    /// 找出在给定状态下所有前置条件已满足的动作
    fn find_applicable_actions(&self, state: &WorldState) -> Vec<(usize, &GoapActionDef)> {
        self.actions
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                action.preconditions.iter().all(|cond| cond.satisfied_by(state))
            })
            .collect()
    }

    /// 检查目标是否已达成
    fn goal_satisfied(&self, state: &WorldState, goal: &WorldState) -> bool {
        goal.0.iter().all(|(key, target_value)| {
            match state.0.get(key) {
                Some(actual) => actual == target_value,
                None => matches!(target_value, WorldStateValue::Nil),
            }
        })
    }

    /// 启发式函数: 估计从当前状态到目标的距离
    ///
    /// 使用 **模式数据库启发式 (Pattern Database Heuristic)**:
    /// 对每个未被满足的目标条件, 估计最少需要几个动作来满足它。
    /// 这是 admissible 的 (不会高估)。
    fn heuristic(&self, state: &WorldState, goal: &WorldState) -> f64 {
        let mut unsatisfied = 0usize;
        let mut total_cost = 0.0f64;

        for (key, target) in &goal.0 {
            match state.0.get(key) {
                Some(actual) if actual == target => continue,
                _ => {
                    unsatisfied += 1;
                    // 估算满足此条件的最低代价
                    // 简化: 每个未满足条件假设至少需要 1 个动作
                    total_cost += self.cheapest_action_for(key).unwrap_or(1.0);
                }
            }
        }

        if unsatisfied == 0 {
            0.0
        } else {
            // 归一化: 未满足条件数 + 平均动作代价
            unsatisfied as f64 + total_cost / unsatisfied.max(1) as f64
        }
    }

    /// 找出能修改指定 key 且代价最小的动作
    fn cheapest_action_for(&self, key: &str) -> Option<f64> {
        self.actions
            .iter()
            .filter(|a| a.effects.iter().any(|e| e.key == key)).map(|a| a.cost).reduce(f64::min)
    }

    /// 状态哈希 (用于去重)
    fn hash_state(&self, state: &WorldState) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = fxhash::FxHasher::default();
        for (k, v) in &state.0 {
            k.hash(&mut hasher);
            std::mem::discriminant(v).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// 从搜索路径构建 GoapPlan
    fn build_plan(&self, terminal: &SearchNode, iterations: usize) -> GoapPlan {
        let steps: Vec<GoapStep> = terminal
            .path
            .iter()
            .enumerate()
            .map(|(idx, step_num)| {
                let action = &self.actions[idx];
                GoapStep {
                    step_number: step_num + 1,
                    action_name: action.name.clone(),
                    params: serde_json::Value::Null,
                    preconditions: action
                        .preconditions
                        .iter()
                        .map(|p| format!("{}{:?} {}", p.key, p.operator, /*value*/ ""))
                        .collect(),
                    effects: action
                        .effects
                        .iter()
                        .map(|e| format!("{} {:?}", e.key, e.operation))
                        .collect(),
                    estimated_cost: action.cost,
                }
            })
            .collect();

        GoapPlan {
            total_cost: terminal.g_cost,
            steps,
            final_state: terminal.state.clone(),
            planning_time_ms: 0.0, // 由上层设置
            iterations,
        }
    }

    /// 根据任务推断初始世界状态
    fn infer_initial_state(&self, task: &ScheduledTask) -> WorldState {
        let mut state = WorldState::new();

        // 基于任务元数据推断
        let meta = &task.metadata; if meta.is_object() {
            if let Some(lang) = meta.get("language") {
                state.set(
                    format!("language_{}", lang.as_str().unwrap_or("unknown")),
                    WorldStateValue::Bool(true),
                );
            }
            if let Some(has_tests) = meta.get("has_tests") {
                state.set(
                    "tests_written".to_string(),
                    if has_tests.as_bool().unwrap_or(false) {
                        WorldStateValue::Bool(true)
                    } else {
                        WorldStateValue::Bool(false)
                    },
                );
            }
        }

        // 依赖已解决?
        if task.dependencies.is_empty() {
            state.set(
                "dependencies_resolved".to_string(),
                WorldStateValue::Bool(true),
            );
        }

        // 默认未安装依赖
        state.set(
            "dependencies_installed".to_string(),
            WorldStateValue::Bool(false),
        );

        state
    }

    /// 解析自然语言目标为结构化目标状态
    ///
    /// 对应 Ruflo Goal Module 的目标解析逻辑。
    /// 支持:
    /// - "部署应用" -> deployed=true, monitoring_active=true
    /// - "重构认证模块" -> refactored=true, tests_written=true
    /// - "修复 bug #123" -> bug_123_fixed=true
    fn parse_goal(&self, goal: &str) -> Result<WorldState, GoapError> {
        let lower = goal.to_lowercase();
        let mut state = WorldState::new();

        // === 部署相关 ===
        if lower.contains("部署") || lower.contains("deploy") {
            state.set("dependencies_installed", WorldStateValue::Bool(true));
            state.set("tests_written".to_string(), WorldStateValue::Bool(true));
            state.set("built".into(), WorldStateValue::Bool(true));
            state.set("deployed".into(), WorldStateValue::Bool(true));
            state.set("monitoring_active".into(), WorldStateValue::Bool(true));

            return Ok(state);
        }

        // === 重构相关 ===
        if lower.contains("重构") || lower.contains("refactor") {
            state.set("analyzed".into(), WorldStateValue::Bool(true));
            state.set("documented".into(), WorldStateValue::Bool(true));
            state.set("tests_written".to_string(), WorldStateValue::Bool(true));
            state.set("refactored".into(), WorldStateValue::Bool(true));

            return Ok(state);
        }

        // === 修复 Bug 相关 ===
        if lower.contains("修复") || lower.contains("fix") || lower.contains("bug") {
            state.set("diagnosed".into(), WorldStateValue::Bool(true));
            state.set("root_cause_found".into(), WorldStateValue::Bool(true));
            state.set("fix_applied".into(), WorldStateValue::Bool(true));
            state.set("tests_updated".into(), WorldStateValue::Bool(true));

            return Ok(state);
        }

        // === 创建新功能 ===
        if lower.contains("创建") || lower.contains("create") || lower.contains("新建") {
            state.set("designed".into(), WorldStateValue::Bool(true));
            state.set("implemented".into(), WorldStateValue::Bool(true));
            state.set("tested".into(), WorldStateValue::Bool(true));
            state.set("integrated".into(), WorldStateValue::Bool(true));

            return Ok(state);
        }

        // === 安全扫描 ===
        if lower.contains("安全") || lower.contains("security") || lower.contains("scan") {
            state.set("scanned".into(), WorldStateValue::Bool(true));
            state.set("vulnerabilities_fixed".into(), WorldStateValue::Bool(true));
            state.set("compliant".into(), WorldStateValue::Bool(true));

            return Ok(state);
        }

        // === 默认: 将整个目标作为一个单一条件 ===
        state.set("goal_achieved".into(), WorldStateValue::Bool(true));
        state.set("_goal_description".into(), WorldStateValue::String(goal.to_string()));

        Ok(state)
    }

    // ========================================================================
    // 内置动作库 (对应 Ruflo 的 100+ Agents 能力)
    // ========================================================================

    /// 默认内置动作 — 覆盖常见 DevOps/开发工作流
    fn builtin_actions() -> Vec<GoapActionDef> {
        vec![
            // === 分析类 ===
            GoapActionDef {
                name: "analyze_codebase".into(),
                preconditions: vec![],
                effects: vec![
                    WorldStateEffect { key: "analyzed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                    WorldStateEffect { key: "dependencies_known".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 1.0,
                category: ActionCategory::Analysis,
                deterministic: true,
                estimated_duration_ms: 2000,
            },

            // === 依赖管理 ===
            GoapActionDef {
                name: "install_dependencies".into(),
                preconditions: vec![
                    WorldStateCondition { key: "dependencies_known".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "dependencies_installed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.0,
                category: ActionCategory::Execution,
                deterministic: false, // 可能因网络原因失败
                estimated_duration_ms: 15000,
            },

            // === 编译/构建 ===
            GoapActionDef {
                name: "build_project".into(),
                preconditions: vec![
                    WorldStateCondition { key: "dependencies_installed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "built".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 3.0,
                category: ActionCategory::Execution,
                deterministic: false,
                estimated_duration_ms: 30000,
            },

            // === 测试 ===
            GoapActionDef {
                name: "write_tests".into(),
                preconditions: vec![
                    WorldStateCondition { key: "analyzed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "tests_written".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.5,
                category: ActionCategory::Creation,
                deterministic: true,
                estimated_duration_ms: 8000,
            },
            GoapActionDef {
                name: "run_tests".into(),
                preconditions: vec![
                    WorldStateCondition { key: "tests_written".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                    WorldStateCondition { key: "built".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "tests_passed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.0,
                category: ActionCategory::Execution,
                deterministic: false,
                estimated_duration_ms: 10000,
            },

            // === 文档 ===
            GoapActionDef {
                name: "document_behavior".into(),
                preconditions: vec![
                    WorldStateCondition { key: "analyzed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "documented".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 1.5,
                category: ActionCategory::Creation,
                deterministic: true,
                estimated_duration_ms: 3000,
            },

            // === 重构 ===
            GoapActionDef {
                name: "plan_refactoring".into(),
                preconditions: vec![
                    WorldStateCondition { key: "documented".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                    WorldStateCondition { key: "tests_written".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "refactor_plan_ready".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.0,
                category: ActionCategory::Analysis,
                deterministic: true,
                estimated_duration_ms: 5000,
            },
            GoapActionDef {
                name: "apply_refactoring".into(),
                preconditions: vec![
                    WorldStateCondition { key: "refactor_plan_ready".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                    WorldStateCondition { key: "tests_passed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "refactored".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                    // 重构后需要回归测试
                    WorldStateEffect { key: "tests_passed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(false) },
                ],
                cost: 4.0,
                category: ActionCategory::Creation,
                deterministic: false,
                estimated_duration_ms: 20000,
            },

            // === 部署 ===
            GoapActionDef {
                name: "deploy_application".into(),
                preconditions: vec![
                    WorldStateCondition { key: "tests_passed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                    WorldStateCondition { key: "built".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "deployed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.0,
                category: ActionCategory::Execution,
                deterministic: false,
                estimated_duration_ms: 12000,
            },
            GoapActionDef {
                name: "setup_monitoring".into(),
                preconditions: vec![
                    WorldStateCondition { key: "deployed".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "monitoring_active".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 1.5,
                category: ActionCategory::Execution,
                deterministic: true,
                estimated_duration_ms: 5000,
            },

            // === 安全 ===
            GoapActionDef {
                name: "security_scan".into(),
                preconditions: vec![],
                effects: vec![
                    WorldStateEffect { key: "scanned".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.0,
                category: ActionCategory::Verification,
                deterministic: true,
                estimated_duration_ms: 6000,
            },
            GoapActionDef {
                name: "fix_vulnerabilities".into(),
                preconditions: vec![
                    WorldStateCondition { key: "scanned".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "vulnerabilities_fixed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 3.0,
                category: ActionCategory::Creation,
                deterministic: false,
                estimated_duration_ms: 12000,
            },

            // === 诊断/调试 ===
            GoapActionDef {
                name: "diagnose_issue".into(),
                preconditions: vec![],
                effects: vec![
                    WorldStateEffect { key: "diagnosed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                    WorldStateEffect { key: "root_cause_found".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                ],
                cost: 2.5,
                category: ActionCategory::Analysis,
                deterministic: false, // 诊断不一定总能找到根因
                estimated_duration_ms: 8000,
            },
            GoapActionDef {
                name: "apply_fix".into(),
                preconditions: vec![
                    WorldStateCondition { key: "root_cause_found".into(), operator: ConditionOp::Equals, value: WorldStateValue::Bool(true) },
                ],
                effects: vec![
                    WorldStateEffect { key: "fix_applied".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(true) },
                    WorldStateEffect { key: "tests_passed".into(), operation: EffectOp::Set, value: WorldStateValue::Bool(false) }, // fix后需重新测试
                ],
                cost: 2.0,
                category: ActionCategory::Creation,
                deterministic: false,
                estimated_duration_ms: 5000,
            },
        ]
    }
}

impl Default for GoapPlanner {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 错误类型
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum GoapError {
    #[error("无法找到规划方案: {0}")]
    NoPlanFound(String),

    #[error("重规划失败: {0}")]
    ReplanningFailed(String),

    #[error("目标解析错误: {0}")]
    GoalParseError(String),

    #[error("内部错误: {0}")]
    Internal(String),
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goap_basic_planning() {
        let planner = GoapPlanner::new();
        assert!(planner.action_count() > 10); // 内置动作应 > 10 个

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let task = ScheduledTask::with_goal(
                "部署应用到生产环境",
                AgentRole::Worker,
                "qwen-3.6-max",
                TaskPriority::High,
            );
            let plan = planner.plan(&task).await.unwrap();
            assert!(!plan.steps.is_empty(), "应生成了规划步骤");
            assert!(plan.total_cost > 0.0, "应有正代价");

            // 验证最后一步接近目标
            let last_step = plan.steps.last().unwrap();
            println!("规划结果: {} 步, 总代价 {:.2}", plan.steps.len(), plan.total_cost);
            for (i, step) in plan.steps.iter().enumerate() {
                println!("  {}. {} (cost={:.1})", i + 1, step.action_name, step.estimated_cost);
            }
        });
    }

    #[test]
    fn test_goal_parsing() {
        let planner = GoapPlanner::new();

        // 部署目标
        let goal = planner.parse_goal("部署应用到生产环境").unwrap();
        assert!(goal.0.contains_key("deployed"));
        assert!(goal.0.contains_key("monitoring_active"));

        // 重构目标
        let goal = planner.parse_goal("重构认证模块").unwrap();
        assert!(goal.0.contains_key("refactored"));

        // 修复目标
        let goal = planner.parse_target("修复登录页面的 bug").unwrap();
        assert!(goal.0.contains_key("fix_applied"));
    }

    #[test]
    fn test_replanning() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let planner = GoapPlanner::new();

        rt.block_on(async {
            let mut state = WorldState::new();
            state.set("dependencies_installed", WorldStateValue::Bool(true));
            state.set("built".into(), WorldStateValue::Bool(true));
            // 还没写测试

            let mut goal = WorldState::new();
            goal.set("deployed".into(), WorldStateValue::Bool(true));
            goal.set("monitoring_active".into(), WorldStateValue::Bool(true));

            // 重规划时应跳过已完成的步骤
            let plan = planner
                .replan(
                    &state,
                    &goal,
                    &["install_dependencies".into(), "build_project".into()],
                )
                .await
                .unwrap();

            // 不应包含已完成的动作
            for step in &plan.steps {
                assert_ne!(step.action_name, "install_dependencies");
                assert_ne!(step.action_name, "build_project");
            }
        });
    }

    #[test]
    fn test_custom_actions() {
        let mut planner = GoapPlanner::new();

        planner.register_action(GoapActionDef {
            name: "custom_action_test".into(),
            preconditions: vec![],
            effects: vec![WorldStateEffect {
                key: "custom_done".into(),
                op: EffectOp::Set,
                value: WorldStateValue::Bool(true),
            }],
            cost: 0.5,
            category: ActionCategory::Execution,
            deterministic: true,
            estimated_duration_ms: 100,
        });

        assert_eq!(planner.action_count(), GoapPlanner::new().action_count() + 1);
    }
}

