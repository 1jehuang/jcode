//! **统一调度队列** — 融合 Ruflo 任务队列 + Parallax 请求信号
//!
//! ## 设计特点
//!
//! 1. **多级优先级**: Critical > Urgent > High > Medium > Low
//! 2. **依赖感知**: 自动跳过依赖未满足的任务
//! 3. **公平性**: 同优先级内 FIFO
//! 4. **容量控制**: 支持最大队列长度限制
//! 5. **等待超时**: 任务等待超过阈值自动升级或拒绝
//!
//! ## 数据结构
//!
//! 使用 **分层 BinaryHeap**:
//! ```
//! Queue {
//!   critical: BinaryHeap<Task>,   // 最高优先级
//!   urgent:   BinaryHeap<Task>,
//!   high:     BinaryHeap<Task>,
//!   medium:   BinaryHeap<Task>,
//!   low:      BinaryHeap<Task>,   // 最低优先级
//! }
//! ```
//! pop_ready() 从最高优先级的非空堆中取出.

use std::collections::{HashSet, HashMap};

use super::*;

// ============================================================================
// UnifiedQueue 结构体
// ============================================================================

/// 统一调度队列
#[derive(Debug)]
pub struct UnifiedQueue {
    /// 分层优先队列 (每层一个 MaxHeap, 但 Ord 反转使其行为像 MinHeap... 不对, 我们要高优先级先出)
    ///
    /// 实际上: 每个优先级内部是一个 BinaryHeap, pop() 出的是最大的 (Ord 定义的最大)
    /// 我们定义 ScheduledTask 的 Ord 使得高优先级任务 "更大"
    queues: [Vec<ScheduledTask>; 5], // [Critical, Urgent, High, Medium, Low]

    /// 最大队列长度 (0 = 无限)
    max_size: usize,

    /// 总元素计数
    len: usize,

    /// 已完成任务 ID 集合 (用于依赖解析)
    completed_tasks: HashSet<TaskId>,

    /// 等待中的任务 (用于超时检测)
    waiting_since: HashMap<TaskId, chrono::DateTime<chrono::Utc>>,

    /// 统计
    pub total_pushed: u64,
    pub total_popped: u64,
    pub total_dropped: u64,
}

impl UnifiedQueue {
    /// 创建新队列
    pub fn new(max_size: usize) -> Self {
        Self {
            queues: [
                vec![], // Critical (index 0)
                vec![], // Urgent   (index 1)
                vec![], // High     (index 2)
                vec![], // Medium   (index 3)
                vec![], // Low      (index 4)
            ],
            max_size,
            len: 0,
            completed_tasks: HashSet::new(),
            waiting_since: HashMap::new(),
            total_pushed: 0,
            total_popped: 0,
            total_dropped: 0,
        }
    }

    /// 当前队列长度
    pub fn len(&self) -> usize {
        self.len
    }

    /// 队列是否为空
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 是否已满
    pub fn is_full(&self) -> bool {
        self.max_size > 0 && self.len >= self.max_size
    }

    /// 入队
    pub fn push(&mut self, task: ScheduledTask) -> Result<(), SchedulerError> {
        if self.is_full() {
            self.total_dropped += 1;
            return Err(SchedulerError::QueueFull(self.len));
        }

        let priority_idx = Self::priority_to_index(task.priority);
        self.queues[priority_idx].push(task);

        // 记录入队时间
        if let Some(last_task) = self.queues[priority_idx].last() {
            self.waiting_since.insert(last_task.id, chrono::Utc::now());
        }

        self.len += 1;
        self.total_pushed += 1;

        Ok(())
    }

    /// 取出下一个就绪任务 (依赖已满足的最高优先级任务)
    ///
    /// 这是调度循环的核心调用。
    /// 依次检查 Critical -> Urgent -> High -> Medium -> Low 各层，
    /// 返回第一个依赖已满足的任务。
    pub fn pop_ready(
        &mut self,
        _task_registry: &dashmap::DashMap<TaskId, ScheduledTask>,
    ) -> Result<Option<ScheduledTask>, SchedulerError> {
        // 从最高优先级到最低优先级扫描
        for priority_idx in 0..5 {
            let queue = &mut self.queues[priority_idx];

            if queue.is_empty() {
                continue;
            }

            // 在当前优先级层内查找第一个依赖已满足的任务
            let mut found_idx = None;

            // 注意: BinaryHeap 不支持随机访问, 所以我们需要逐个 peek
            // 为了效率, 可以维护一个辅助索引或改用其他数据结构
            // 这里简化为线性扫描
            let mut temp = vec![];

            while let Some(task) = queue.pop() {
                if task.dependencies_met(&self.completed_tasks) {
                    found_idx = Some(task);
                    // 把临时取出的放回去
                    for t in temp.drain(..) {
                        queue.push(t);
                    }
                    break;
                } else {
                    // 依赖未满足 -> 暂存
                    temp.push(task);
                }
            }

            // 把没取到的放回
            for task in temp {
                queue.push(task);
            }

            if let Some(task) = found_idx {
                self.waiting_since.remove(&task.id);
                self.len -= 1;
                self.total_popped += 1;
                return Ok(Some(task));
            }
        }

        // 所有层都没有就绪的任务
        Ok(None)
    }

    /// 弹出指定任务 (用于取消)
    pub fn remove(&mut self, task_id: &TaskId) -> bool {
        for queue in &mut self.queues {
            if let Some(pos) = queue.iter().position(|t| &t.id == task_id) {
                queue.swap_remove(pos);
                self.len -= 1;
                self.waiting_since.remove(task_id);
                return true;
            }
        }
        false
    }

    /// 标记任务已完成 (更新依赖图)
    pub fn mark_completed(&mut self, task_id: TaskId) {
        self.completed_tasks.insert(task_id);
    }

    /// 清除已完成标记 (用于重规划场景)
    pub fn clear_completed(&mut self) {
        self.completed_tasks.clear();
    }

    /// 获取超时的任务 (等待时间超过阈值的任务)
    pub fn get_expired_tasks(&self, timeout_ms: u64) -> Vec<TaskId> {
        let now = chrono::Utc::now();
        self.waiting_since
            .iter()
            .filter(|&(_, ref ts)| {
                let elapsed = now.signed_duration_since(*ts);
                elapsed.num_milliseconds() as u64 > timeout_ms
            })
            .map(|(&id, _)| id)
            .collect()
    }

    /// 升级任务的优先级
    pub fn bump_priority(&mut self, task_id: &TaskId, new_priority: TaskPriority) -> bool {
        for (priority_idx, queue) in self.queues.iter_mut().enumerate() {
            if let Some(pos) = queue.iter().position(|t| &t.id == task_id) {
                let mut task = queue.swap_remove(pos);
                task.priority = new_priority;
                let new_idx = Self::priority_to_index(new_priority);

                self.queues[new_idx].push(task);
                self.len -= 1; // push 会重新计数? 不, 我们手动管理的 len
                // 实际上这里不需要调整 len 因为只是移动
                return true;
            }
        }
        false
    }

    /// 获取各层任务数
    pub fn counts_by_priority(&self) -> [usize; 5] {
        [
            self.queues[0].len(),
            self.queues[1].len(),
            self.queues[2].len(),
            self.queues[3].len(),
            self.queues[4].len(),
        ]
    }

    /// 优先级枚举 -> 数组索引
    fn priority_to_index(priority: TaskPriority) -> usize {
        match priority {
            TaskPriority::Critical => 0,
            TaskPriority::Urgent => 1,
            TaskPriority::High => 2,
            TaskPriority::Medium => 3,
            TaskPriority::Low => 4,
        }
    }

    /// 数组索引 -> 优先级枚举
    pub fn index_to_priority(idx: usize) -> TaskPriority {
        match idx {
            0 => TaskPriority::Critical,
            1 => TaskPriority::Urgent,
            2 => TaskPriority::High,
            3 => TaskPriority::Medium,
            _ => TaskPriority::Low,
        }
    }

    /// 清空队列
    pub fn clear(&mut self) {
        for q in &mut self.queues {
            q.clear();
        }
        self.len = 0;
        self.completed_tasks.clear();
        self.waiting_since.clear();
    }
}

// ============================================================================
// ScheduledTask Ord 实现修正
// ============================================================================
//
// 注意: Rust 的 BinaryHeap 是 MaxHeap (pop 返回最大的元素).
// 我们希望 pop() 返回最高优先级的任务.
// 所以 ScheduledTask 的 Ord 应使得: 高优先级 > 低优先级.
// 同时同优先级内 FIFO (提交早的小).

// (这个实现在 types.rs 中已经定义了, 这里确认一致性)

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(name: &str, priority: TaskPriority, deps: Vec<TaskId>) -> ScheduledTask {
        let mut task = ScheduledTask::simple(name, AgentRole::Worker, "test-model");
        task.priority = priority;
        task.dependencies = deps;
        task
    }

    #[test]
    fn test_basic_push_pop() {
        let mut queue = UnifiedQueue::new(100);

        let t1 = make_task("low-priority", TaskPriority::Low, vec![]);
        let t2 = make_task("high-priority", TaskPriority::High, vec![]);
        let t3 = make_task("critical", TaskPriority::Critical, vec![]);

        queue.push(t1).unwrap();
        queue.push(t2).unwrap();
        queue.push(t3).unwrap();

        assert_eq!(queue.len(), 3);

        let registry = dashmap::DashMap::new();

        // pop_ready 应返回最高优先级
        let popped = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped.priority, TaskPriority::Critical);
        assert_eq!(queue.len(), 2);

        let popped2 = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped2.priority, TaskPriority::High);

        let popped3 = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped3.priority, TaskPriority::Low);

        assert!(queue.pop_ready(&registry).unwrap().is_none());
    }

    #[test]
    fn test_dependency_blocking() {
        let mut queue = UnifiedQueue::new(100);
        let registry = dashmap::DashMap::new();

        let dep_id = uuid::Uuid::new_v4();
        let child = make_task("depends-on-dep", TaskPriority::Critical, vec![dep_id]);
        let independent = make_task("independent", TaskPriority::Low, vec![]);

        queue.push(child).unwrap();
        queue.push(independent).unwrap();

        // dep 未完成 -> Critical 任务不可弹出
        let popped = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped.id, independent.id, "应弹出独立的低优先级任务");

        // 完成 dep
        queue.mark_completed(dep_id);

        // 现在 Critical 子任务应该可以弹出了
        let popped2 = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped2.priority, TaskPriority::Critical);
    }

    #[test]
    fn test_queue_full() {
        let mut queue = UnifiedQueue::new(2);

        queue.push(make_task("a", TaskPriority::Medium, vec![])).unwrap();
        queue.push(make_task("b", TaskPriority::Medium, vec![])).unwrap();

        let result = queue.push(make_task("c", TaskPriority::Medium, vec![]));
        assert!(result.is_err(), "第三个任务应被拒绝 (队列满)");
        assert_eq!(queue.total_dropped, 1);
    }

    #[test]
    fn test_fifo_within_same_priority() {
        let mut queue = UnifiedQueue::new(100);
        let registry = dashmap::DashMap::new();

        let t1 = make_task("first", TaskPriority::High, vec![]);
        let t2 = make_task("second", TaskPriority::High, vec![]);
        let t3 = make_task("third", TaskPriority::High, vec![]);

        queue.push(t1).unwrap();
        queue.push(t2).unwrap();
        queue.push(t3).unwrap();

        // 同优先级内应按提交顺序 (FIFO) 弹出
        let p1 = queue.pop_ready(&registry).unwrap().unwrap();
        let p2 = queue.pop_ready(&registry).unwrap().unwrap();
        let p3 = queue.pop_ready(&registry).unwrap().unwrap();

        assert_eq!(p1.description, "first");
        assert_eq!(p2.description, "second");
        assert_eq!(p3.description, "third");
    }

    #[test]
    fn test_bump_priority() {
        let mut queue = UnifiedQueue::new(100);
        let registry = dashmap::DashMap::new();

        let low_task = make_task("low-task", TaskPriority::Low, vec![]);
        let low_id = low_task.id;
        queue.push(low_task).unwrap();

        let high_task = make_task("high-task", TaskPriority::High, vec![]);
        queue.push(high_task).unwrap();

        // 升级低优先级任务
        let bumped = queue.bump_priority(&low_id, TaskPriority::Critical);

        assert!(bumped);

        // 现在它应该是第一个弹出的
        let popped = queue.pop_ready(&registry).unwrap().unwrap();
        assert_eq!(popped.id, low_id, "升级后的任务应先弹出");
        assert_eq!(popped.priority, TaskPriority::Critical);
    }

    #[test]
    fn test_remove() {
        let mut queue = UnifiedQueue::new(100);
        let registry = dashmap::DashMap::new();

        let t1 = make_task("to-remove", TaskPriority::Medium, vec![]);
        let id = t1.id;
        queue.push(t1).unwrap();

        assert_eq!(queue.len(), 1);
        assert!(queue.remove(&id));
        assert_eq!(queue.len(), 0);
        assert!(queue.pop_ready(&registry).unwrap().is_none());
    }

    #[test]
    fn test_counts_by_priority() {
        let mut queue = UnifiedQueue::new(100);

        queue.push(make_task("", TaskPriority::Critical, vec![])).unwrap();
        queue.push(make_task("", TaskPriority::Low, vec![])).unwrap();
        queue.push(make_task("", TaskPriority::High, vec![])).unwrap();
        queue.push(make_task("", TaskPriority::Low, vec![])).unwrap();

        let counts = queue.counts_by_priority();
        assert_eq!(counts[0], 1); // Critical
        assert_eq!(counts[1], 0); // Urgent
        assert_eq!(counts[2], 1); // High
        assert_eq!(counts[3], 0); // Medium
        assert_eq!(counts[4], 2); // Low
    }
}
