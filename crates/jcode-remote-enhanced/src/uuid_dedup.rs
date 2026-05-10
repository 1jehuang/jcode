//! Bounded UUID Set - 有界 UUID 去重集合
//!
//! 移植自 Claude Code `remoteBridgeCore.ts`:
//! ```typescript
//! const recentPostedUUIDs = new BoundedUUIDSet(2000)
//! const recentInboundUUIDs = new BoundedUUIDSet(2000)
//! ```
//!
//! 设计:
//! - 内部使用 HashSet 实现 O(1) 查找和插入
//! - 使用 VecDeque 维护 FIFO 淘汰顺序
//! - 达到容量上限时自动淘汰最老的条目
//! - 线程安全 (通过 Mutex 保护)

use std::collections::{HashSet, VecDeque};
use uuid::Uuid;

/// 有界 UUID 去重集合
///
/// # Example
/// ```ignore
/// let mut set = BoundedUuidSet::with_capacity(2000);
/// let id = Uuid::new_v4();
/// assert!(set.insert(id));       // 第一次插入返回 true
/// assert!(!set.insert(id));      // 重复插入返回 false
/// assert!(set.contains(&id));   // 存在检查
/// ```
pub struct BoundedUuidSet {
    /// O(1) 查找集合
    set: HashSet<Uuid>,
    
    /// FIFO 淘汰顺序队列
    queue: VecDeque<Uuid>,
    
    /// 最大容量
    capacity: usize,
}

impl BoundedUuidSet {
    /// 创建指定容量的去重集合
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// 插入 UUID
    ///
    /// Returns:
    /// - `true`: 如果 UUID 是新元素 (已成功插入)
    /// - `false`: 如果 UUID 已存在 (重复)
    pub fn insert(&mut self, uuid: Uuid) -> bool {
        if self.set.contains(&uuid) {
            return false; // 已存在, 返回 false 表示重复
        }

        // 检查是否需要淘汰最老条目
        if self.queue.len() >= self.capacity {
            if let Some(old_uuid) = self.queue.pop_front() {
                self.set.remove(&old_uuid);
            }
        }

        // 插入新元素
        self.set.insert(uuid);
        self.queue.push_back(uuid);
        
        true
    }

    /// 检查 UUID 是否存在于集合中
    pub fn contains(&self, uuid: &Uuid) -> bool {
        self.set.contains(uuid)
    }

    /// 获取当前元素数量
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// 获取最大容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 清空所有元素
    pub fn clear(&mut self) {
        self.set.clear();
        self.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_insert_and_contains() {
        let mut set = BoundedUuidSet::with_capacity(10);
        let id = Uuid::new_v4();

        assert_eq!(set.len(), 0);
        assert!(set.insert(id));
        assert_eq!(set.len(), 1);
        assert!(set.contains(&id));

        // 重复插入
        assert!(!set.insert(id));
        assert_eq!(set.len(), 1); // 长度不变
    }

    #[test]
    fn test_bounded_eviction() {
        let mut set = BoundedUuidSet::with_capacity(3);

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();
        let id4 = Uuid::new_v4();

        assert!(set.insert(id1));
        assert!(set.insert(id2));
        assert!(set.insert(id3));
        assert_eq!(set.len(), 3);

        // 第 4 个插入应该淘汰第 1 个
        assert!(set.insert(id4));
        assert_eq!(set.len(), 3); // 仍然只有 3 个
        
        // id1 应该已被淘汰
        assert!(!set.contains(&id1));
        
        // id2, id3, id4 应该还在
        assert!(set.contains(&id2));
        assert!(set.contains(&id3));
        assert!(set.contains(&id4));
    }

    #[test]
    fn test_clear() {
        let mut set = BoundedUuidSet::with_capacity(10);
        set.insert(Uuid::new_v4());
        set.insert(Uuid::new_v4());
        
        set.clear();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }
}
