//! # Version Vector 实现
//!
//! 实现了版本向量 (Version Vector)，用于分布式系统中的因果一致性追踪。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::cmp::Ordering;
use serde::{Deserialize, Serialize};

/// 版本向量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionVector(HashMap<String, u64>);

impl VersionVector {
    /// 创建新的空版本向量
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// 增加指定节点的版本号
    pub fn increment(&mut self, node_id: &str) -> u64 {
        let counter = self.0.entry(node_id.to_string()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// 获取指定节点的版本号
    pub fn get(&self, node_id: &str) -> u64 {
        self.0.get(node_id).copied().unwrap_or(0)
    }

    /// 合并两个版本向量
    pub fn merge(&self, other: &VersionVector) -> VersionVector {
        let mut result = self.0.clone();
        for (k, v) in &other.0 {
            let entry = result.entry(k.clone()).or_insert(0);
            *entry = (*v).max(*entry);
        }
        Self(result)
    }

    /// 检查版本向量 A 是否发生在版本向量 B 之前
    /// A happened_before B 当且仅当：
    /// - 对于所有节点，A 的版本号 <= B 的版本号
    /// - 至少有一个节点，A 的版本号 < B 的版本号
    pub fn happened_before(&self, other: &VersionVector) -> bool {
        // 如果 self == other，返回 false
        if self == other {
            return false;
        }

        // 检查 self 中的每个条目
        for (node, counter) in &self.0 {
            let other_counter = other.0.get(node).copied().unwrap_or(0);
            if *counter > other_counter {
                return false;
            }
        }

        // 检查 other 中有但 self 中没有的节点
        for (node, counter) in &other.0 {
            if !self.0.contains_key(node) && *counter > 0 {
                return false;
            }
        }

        // 至少有一个严格小于
        for (node, counter) in &self.0 {
            let other_counter = other.0.get(node).copied().unwrap_or(0);
            if *counter < other_counter {
                return true;
            }
        }

        // 如果到这里，说明所有条目都相等但 self != other（不可能的情况）或者 other 有额外的条目
        for (node, counter) in &other.0 {
            if !self.0.contains_key(node) && *counter > 0 {
                return true;
            }
        }

        false
    }

    /// 检查两个版本向量是否并发
    pub fn is_concurrent(&self, other: &VersionVector) -> bool {
        !self.happened_before(other) && !other.happened_before(self)
    }

    /// 检查 self 是否等于 other
    pub fn equals(&self, other: &VersionVector) -> bool {
        self == other
    }

    /// 获取所有节点 ID
    pub fn nodes(&self) -> Vec<&String> {
        self.0.keys().collect()
    }

    /// 获取条目数量
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 转换为向量时钟格式 (用于调试/日志)
    pub fn to_vector_clock(&self) -> Vec<(String, u64)> {
        let mut items: Vec<_> = self.0.iter().map(|(k, v)| (k.clone(), *v)).collect();
        items.sort_by(|(a, _), (b, _)| a.cmp(b));
        items
    }

    /// 从向量时钟格式创建
    pub fn from_vector_clock(items: &[(String, u64)]) -> Self {
        let mut vv = Self::new();
        for (node, counter) in items {
            vv.0.insert(node.clone(), *counter);
        }
        vv
    }
}

impl Default for VersionVector {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for VersionVector {
    fn eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            return false;
        }
        for (node, counter) in &self.0 {
            match other.0.get(node) {
                Some(other_counter) if *counter == *other_counter => continue,
                _ => return false,
            }
        }
        true
    }
}

impl Eq for VersionVector {}

impl Hash for VersionVector {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut items: Vec<_> = self.0.iter().collect();
        items.sort_by(|(a, _), (b, _)| a.cmp(b));
        items.hash(state);
    }
}

impl std::fmt::Display for VersionVector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let items: Vec<String> = self.0
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect();
        write!(f, "[{}]", items.join(", "))
    }
}

/// 版本向量比较结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionRelation {
    /// self 发生在 other 之前
    Before,
    /// self 发生在 other 之后
    After,
    /// self 和 other 并发
    Concurrent,
    /// self 和 other 相等
    Equal,
}

impl VersionVector {
    /// 比较两个版本向量
    pub fn compare(&self, other: &VersionVector) -> VersionRelation {
        if self == other {
            return VersionRelation::Equal;
        }
        if self.happened_before(other) {
            return VersionRelation::Before;
        }
        if other.happened_before(self) {
            return VersionRelation::After;
        }
        VersionRelation::Concurrent
    }
}

/// 版本向量工厂
pub struct VersionVectorFactory;

impl VersionVectorFactory {
    /// 创建一个单一节点的版本向量
    pub fn single(node_id: &str) -> VersionVector {
        let mut vv = VersionVector::new();
        vv.increment(node_id);
        vv
    }

    /// 创建一个空的版本向量
    pub fn empty() -> VersionVector {
        VersionVector::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_increment() {
        let mut vv = VersionVector::new();
        assert_eq!(vv.increment("node1"), 1);
        assert_eq!(vv.increment("node1"), 2);
        assert_eq!(vv.increment("node2"), 1);
        assert_eq!(vv.get("node1"), 2);
        assert_eq!(vv.get("node2"), 1);
        assert_eq!(vv.get("node3"), 0);
    }

    #[test]
    fn test_merge() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        vv1.increment("node1");
        vv1.increment("node2");

        let mut vv2 = VersionVector::new();
        vv2.increment("node1");
        vv2.increment("node3");

        let merged = vv1.merge(&vv2);

        assert_eq!(merged.get("node1"), 2); // max(2, 1) = 2
        assert_eq!(merged.get("node2"), 1); // only in vv1
        assert_eq!(merged.get("node3"), 1); // only in vv2
    }

    #[test]
    fn test_happened_before() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        vv1.increment("node1");

        let mut vv2 = VersionVector::new();
        vv2.increment("node1");
        vv2.increment("node1");
        vv2.increment("node2");

        assert!(vv1.happened_before(&vv2));
        assert!(!vv2.happened_before(&vv1));
    }

    #[test]
    fn test_is_concurrent() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");

        let mut vv2 = VersionVector::new();
        vv2.increment("node2");

        assert!(vv1.is_concurrent(&vv2));
        assert!(vv2.is_concurrent(&vv1));

        // vv1 和 vv2 是并发的，因为它们在不同的分支上发展
    }

    #[test]
    fn test_compare() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");

        let mut vv2 = VersionVector::new();
        vv2.increment("node1");

        assert_eq!(vv1.compare(&vv2), VersionRelation::Equal);

        vv2.increment("node1");
        assert_eq!(vv1.compare(&vv2), VersionRelation::Before);
        assert_eq!(vv2.compare(&vv1), VersionRelation::After);

        let mut vv3 = VersionVector::new();
        vv3.increment("node2");
        assert_eq!(vv1.compare(&vv3), VersionRelation::Concurrent);
    }

    #[test]
    fn test_concurrent_branches() {
        // 模拟两个并发分支
        let mut branch_a = VersionVector::new();
        branch_a.increment("node1");
        branch_a.increment("node1"); // A 在 node1 上做了两次操作

        let mut branch_b = VersionVector::new();
        branch_b.increment("node2");
        branch_b.increment("node2"); // B 在 node2 上做了两次操作

        // A 和 B 是并发的
        assert!(branch_a.is_concurrent(&branch_b));

        // 现在让 B 合并到 A
        let merged = branch_a.merge(&branch_b);
        assert_eq!(merged.get("node1"), 2);
        assert_eq!(merged.get("node2"), 2);

        // 合并后的版本发生在 A 和 B 之前
        assert!(branch_a.happened_before(&merged));
        assert!(branch_b.happened_before(&merged));
    }

    #[test]
    fn test_empty_version() {
        let empty = VersionVector::new();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let mut vv = VersionVector::new();
        vv.increment("node1");

        assert!(!vv.is_empty());
        assert_eq!(vv.len(), 1);
    }

    #[test]
    fn test_to_and_from_vector_clock() {
        let mut vv = VersionVector::new();
        vv.increment("node1");
        vv.increment("node1");
        vv.increment("node2");

        let clock = vv.to_vector_clock();
        assert_eq!(clock, vec![("node1".to_string(), 2), ("node2".to_string(), 1)]);

        let restored = VersionVector::from_vector_clock(&clock);
        assert_eq!(restored, vv);
    }

    #[test]
    fn test_display() {
        let mut vv = VersionVector::new();
        vv.increment("node1");
        vv.increment("node2");

        let display = format!("{}", vv);
        assert!(display.contains("node1"));
        assert!(display.contains("node2"));
    }

    #[test]
    fn test_factory() {
        let single = VersionVectorFactory::single("node1");
        assert_eq!(single.get("node1"), 1);
        assert_eq!(single.len(), 1);

        let empty = VersionVectorFactory::empty();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_complex_scenario() {
        // 模拟一个更复杂的场景：
        // 1. 三个节点初始状态相同
        // 2. node1 和 node2 并发地进行了操作
        // 3. node3 基于 node1 的状态进行了操作
        // 4. node1 和 node2 后来进行了同步

        let mut base = VersionVector::new();
        base.increment("node1");
        base.increment("node2");
        base.increment("node3");

        // node1 的分支
        let mut node1_branch = base.clone();
        node1_branch.increment("node1");
        node1_branch.increment("node1"); // node1 又做了操作

        // node2 的并发分支
        let mut node2_branch = base.clone();
        node2_branch.increment("node2");
        node2_branch.increment("node2");

        // node1_branch 和 node2_branch 是并发的
        assert!(node1_branch.is_concurrent(&node2_branch));

        // node3 基于 node1 的状态
        let mut node3_branch = node1_branch.clone();
        node3_branch.increment("node3");

        // node3 发生在 node1_branch 之后
        assert!(node1_branch.happened_before(&node3_branch));

        // 同步：node1_branch 和 node2_branch 合并
        let synced = node1_branch.merge(&node2_branch);

        // 同步后的版本发生在两个分支之后
        assert!(node1_branch.happened_before(&synced));
        assert!(node2_branch.happened_before(&synced));

        // node3 也应该发生在同步版本之前（因为它只基于 node1_branch）
        assert!(node3_branch.happened_before(&synced));
    }
}
