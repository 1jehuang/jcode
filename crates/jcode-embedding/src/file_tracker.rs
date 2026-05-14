//! File Activity Tracker - 文件活动追踪器
//!
//! 追踪用户的文件访问和编辑模式，用于：
//! - 上下文相关性计算
//! - 热点文件识别
//! - 共现关系分析 (co-occurrence)
//!
//! 核心功能：
//! - 记录文件访问/编辑事件
//! - 计算热度分数 (时间衰减函数)
//! - 维护共现矩阵
//! - 推荐相关文件

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// 文件活动记录
#[derive(Debug, Clone)]
pub struct FileActivityRecord {
    /// 文件路径
    pub file_path: PathBuf,
    
    /// 最后一次访问时间
    pub last_accessed: Instant,
    
    /// 总访问次数
    pub access_count: u64,
    
    /// 总编辑次数
    pub edit_count: u64,
    
    /// 当前热度分数 (基于时间衰减)
    hotness_score: f64,
    
    /// 相关文件列表 (通过共现统计得出)
    related_files: Vec<PathBuf>,
    
    /// 创建时间
    created_at: Instant,
}

impl FileActivityRecord {
    fn new(file_path: impl Into<PathBuf>) -> Self {
        let now = Instant::now();
        Self {
            file_path: file_path.into(),
            last_accessed: now,
            access_count: 0,
            edit_count: 0,
            hotness_score: 1.0,
            related_files: Vec::new(),
            created_at: now,
        }
    }

    /// 更新热度分数 (指数衰减)
    fn update_hotness(&mut self) {
        let elapsed = self.last_accessed.elapsed().as_secs_f64();
        
        // 半衰期: 30 分钟 (1800 秒)
        // 公式: score = base * e^(-t / half_life)
        let half_life = 1800.0; // 30 分钟
        
        // 基础分数 = 访问次数 + 编辑次数 * 2 (编辑权重更高)
        let base_score = self.access_count as f64 + (self.edit_count as f64 * 2.0);
        
        // 应用时间衰减
        let decay = (-elapsed / half_life).exp();
        self.hotness_score = base_score * decay;
        
        // 最小值保护 (避免完全归零)
        if self.hotness_score < 0.01 {
            self.hotness_score = 0.01;
        }
    }
}

/// 相关性分数结果
#[derive(Debug, Clone)]
pub struct RelevanceScore {
    /// 文件路径
    pub file_path: PathBuf,
    
    /// 综合相关性分数 (0.0 - 1.0)
    pub relevance: f64,
    
    /// 分数组成明细
    pub breakdown: RelevanceBreakdown,
}

/// 相关性分数明细
#[derive(Debug, Clone)]
pub struct RelevanceBreakdown {
    /// 活动度分数 (0.0 - 1.0)
    pub activity_score: f64,
    
    /// 共现分数 (0.0 - 1.0)
    pub co_occurrence_score: f64,
    
    /// 路径邻近度分数 (0.0 - 1.0)
    pub proximity_score: f64,
}

/// 文件活动追踪器
pub struct FileActivityTracker {
    /// 所有文件的活跃记录
    activities: Arc<RwLock<HashMap<PathBuf, FileActivityRecord>>>,
    
    /// 共现矩阵: (file_a, file_b) -> 共现次数
    co_occurrence_matrix: Arc<RwLock<HashMap<(PathBuf, PathBuf), u64>>>,
    
    /// 最近活跃的文件窗口 (滑动窗口大小)
    recent_window: Arc<RwLock<VecDeque<(PathBuf, Instant)>>>,
    
    /// 配置参数
    config: ActivityConfig,
}

use std::collections::VecDeque;

/// 配置参数
#[derive(Debug, Clone)]
pub struct ActivityConfig {
    /// 共现窗口大小 (最近 N 个文件视为"同时活跃")
    pub co_occurrence_window_size: usize,
    
    /// 衰减半衰期 (秒)
    pub decay_half_life_secs: f64,
    
    /// 最大记录的文件数量
    pub max_tracked_files: usize,
    
    /// 清理间隔 (多久清理一次过期数据)
    pub cleanup_interval_secs: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            co_occurrence_window_size: 10, // 最近 10 个活跃文件
            decay_half_life_secs: 1800.0, // 30 分钟
            max_tracked_files: 10_000, // 最多追踪 10000 个文件
            cleanup_interval_secs: 300, // 每 5 分钟清理一次
        }
    }
}

impl FileActivityTracker {
    /// 创建新的文件活动追踪器
    pub fn new(config: ActivityConfig) -> Self {
        Self {
            activities: Arc::new(RwLock::new(HashMap::new())),
            co_occurrence_matrix: Arc::new(RwLock::new(HashMap::new())),
            recent_window: Arc::new(RwLock::new(VecDeque::with_capacity(
                config.co_occurrence_window_size * 2,
            ))),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(ActivityConfig::default())
    }

    /// 记录文件访问事件
    pub fn record_access(&self, file_path: &Path) {
        let path = file_path.to_path_buf();
        
        // 更新或创建活动记录
        {
            let mut activities = self.activities.write();
            let record = activities.entry(path.clone())
                .or_insert_with(|| FileActivityRecord::new(&path));
            
            record.last_accessed = Instant::now();
            record.access_count += 1;
            record.update_hotness();
        }
        
        // 更新共现矩阵
        self.update_co_occurrence(&path);
        
        // 更新最近窗口
        {
            let mut window = self.recent_window.write();
            window.push_back((path.clone(), Instant::now()));
            
            // 保持窗口大小
            while window.len() > self.config.co_occurrence_window_size {
                window.pop_front();
            }
        }
        
        debug!(file = %path.display(), "File access recorded");
    }

    /// 记录文件编辑事件 (权重高于访问)
    pub fn record_edit(&self, file_path: &Path) {
        let path = file_path.to_path_buf();
        
        // 更新活动记录
        {
            let mut activities = self.activities.write();
            let record = activities.entry(path.clone())
                .or_insert_with(|| FileActivityRecord::new(&path));
            
            record.last_accessed = Instant::now();
            record.edit_count += 1;
            record.update_hotness(); // 编辑会显著提升热度
        }
        
        // 同样更新共现和窗口
        self.update_co_occurrence(&path);
        
        {
            let mut window = self.recent_window.write();
            window.push_back((path.clone(), Instant::now()));

            while window.len() > self.config.co_occurrence_window_size {
                window.pop_front();
            }
        }

        info!(file = %path.display(), "File edit recorded");
    }

    /// 获取当前最热的 N 个文件 (按热度排序)
    pub fn get_hottest_files(&self, limit: usize) -> Vec<(PathBuf, f64)> {
        let mut activities = self.activities.write();

        let mut scored: Vec<_> = activities.iter_mut()
            .map(|(path, record)| {
                record.update_hotness();
                (path.clone(), record.hotness_score)
            })
            .collect();
        
        // 按热度降序排序
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        scored.into_iter().take(limit).collect()
    }

    /// 获取与当前文件相关的文件列表 (用于上下文检索)
    pub fn get_relevant_files(
        &self,
        current_file: &Path,
        limit: usize,
    ) -> Vec<RelevanceScore> {
        let activities = self.activities.read();
        
        let mut scored: Vec<RelevanceScore> = activities
            .iter()
            .filter(|(path, _)| *path != current_file) // 排除当前文件本身
            .map(|(path, record)| {
                let activity_score = self.normalize_hotness(record.hotness_score);
                let co_occurrence_score = self.get_co_occurrence_score(current_file, path);
                let proximity_score = self.calculate_path_proximity(current_file, path);
                
                let total_relevance = 
                    activity_score * 0.4 +      // 活动度权重 40%
                    co_occurrence_score * 0.3 + // 共现权重 30%
                    proximity_score * 0.3;     // 邻近度权重 30%
                
                RelevanceScore {
                    file_path: path.clone(),
                    relevance: total_relevance,
                    breakdown: RelevanceBreakdown {
                        activity_score,
                        co_occurrence_score,
                        proximity_score,
                    },
                }
            })
            .filter(|score| score.relevance > 0.05) // 过滤低相关性的文件
            .collect();
        
        // 按相关性降序排序
        scored.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap());
        
        scored.into_iter().take(limit).collect()
    }

    /// 获取文件的完整活动记录
    pub fn get_file_record(&self, file_path: &Path) -> Option<FileActivityRecord> {
        let activities = self.activities.read();
        activities.get(file_path).cloned()
    }

    /// 获取所有被追踪的文件总数
    pub fn tracked_file_count(&self) -> usize {
        self.activities.read().len()
    }

    /// 获取统计摘要
    pub fn get_stats(&self) -> ActivityStats {
        let mut activities = self.activities.write();
        let co_occurrence = self.co_occurrence_matrix.read();

        let total_accesses: u64 = activities.values()
            .map(|r| r.access_count)
            .sum();

        let total_edits: u64 = activities.values()
            .map(|r| r.edit_count)
            .sum();

        let hottest = activities.values_mut()
            .map(|r| { r.update_hotness(); r.hotness_score })
            .fold(0.0f64, |max, val| max.max(val));

        ActivityStats {
            tracked_files: activities.len(),
            total_accesses,
            total_edits,
            avg_hotness: if activities.is_empty() { 0.0 } else {
                let sum: f64 = activities.values_mut()
                    .map(|r| { r.update_hotness(); r.hotness_score })
                    .sum();
                sum / activities.len() as f64
            },
            max_hotness: hottest,
            co_occurrence_pairs: co_occurrence.len(),
        }
    }

    /// 清理过期数据 (定期调用)
    pub fn cleanup_expired(&self) {
        let max_age = Duration::from_secs(self.config.decay_half_life_secs as u64 * 6); // 3 个半衰期
        
        let before_count;
        {
            let mut activities = self.activities.write();
            before_count = activities.len();
            
            // 移除长时间未访问且热度极低的文件
            activities.retain(|_, record| {
                record.created_at.elapsed() < max_age || record.hotness_score > 0.001
            });
        }
        
        let after_count = self.activities.read().len();
        
        if before_count != after_count {
            info!(
                removed = before_count - after_count,
                remaining = after_count,
                "Cleaned up expired file records"
            );
        }
    }

    // === 内部辅助方法 ===

    /// 更新共现矩阵
    fn update_co_occurrence(&self, current_file: &Path) {
        let window = self.recent_window.read();
        
        for (other_file, _) in window.iter() {
            if other_file != current_file {
                let mut matrix = self.co_occurrence_matrix.write();
                
                // 确保键的顺序一致 (避免重复)
                let key = if current_file < other_file.as_path() {
                    (current_file.to_path_buf(), other_file.clone())
                } else {
                    (other_file.clone(), current_file.to_path_buf())
                };
                
                *matrix.entry(key).or_insert(0) += 1;
            }
        }
    }

    /// 获取两个文件的共现分数 (归一化到 0-1)
    fn get_co_occurrence_score(&self, file_a: &Path, file_b: &Path) -> f64 {
        let matrix = self.co_occurrence_matrix.read();
        
        // 确保键的顺序一致
        let key = if file_a < file_b {
            (file_a.to_path_buf(), file_b.to_path_buf())
        } else {
            (file_b.to_path_buf(), file_a.to_path_buf())
        };
        
        match matrix.get(&key) {
            Some(&count) => {
                // 归一化: 假设最大共现次数为 20 (可配置)
                (count as f64 / 20.0).min(1.0)
            }
            None => 0.0,
        }
    }

    /// 计算路径邻近度 (同一目录或相邻目录得分高)
    fn calculate_path_proximity(&self, file_a: &Path, file_b: &Path) -> f64 {
        // 提取父目录
        let parent_a = file_a.parent().unwrap_or(Path::new(""));
        let parent_b = file_b.parent().unwrap_or(Path::new(""));
        
        if parent_a == parent_b {
            // 同一目录
            return 1.0;
        }
        
        // 计算公共前缀深度
        let components_a: Vec<_> = parent_a.components().collect();
        let components_b: Vec<_> = parent_b.components().collect();
        
        let common_depth = components_a
            .iter()
            .zip(components_b.iter())
            .take_while(|(a, b)| a == b)
            .count();
        
        // 归一化: 公共深度 / 最大可能深度
        let max_depth = components_a.len().max(components_b.len()).max(1);
        common_depth as f64 / max_depth as f64
    }

    /// 归一化热度分数到 0-1 范围
    fn normalize_hotness(&self, raw_score: f64) -> f64 {
        // 使用对数缩放处理长尾分布
        (raw_score.ln() / 10.0).min(1.0).max(0.0)
    }
}

/// 统计摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityStats {
    /// 被追踪的文件总数
    pub tracked_files: usize,
    
    /// 总访问次数
    pub total_accesses: u64,
    
    /// 总编辑次数
    pub total_edits: u64,
    
    /// 平均热度分数
    pub avg_hotness: f64,
    
    /// 最高热度分数
    pub max_hotness: f64,
    
    /// 共现文件对数量
    pub co_occurrence_pairs: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_record_access() {
        let tracker = FileActivityTracker::with_defaults();
        
        let file1 = Path::new("src/main.rs");
        let file2 = Path::new("src/lib.rs");
        
        tracker.record_access(file1);
        tracker.record_access(file2);
        tracker.record_access(file1); // 再次访问
        
        assert_eq!(tracker.tracked_file_count(), 2);
        
        let record = tracker.get_file_record(file1).unwrap();
        assert_eq!(record.access_count, 2);
        assert_eq!(record.edit_count, 0);
    }

    #[test]
    fn test_record_edit() {
        let tracker = FileActivityTracker::with_defaults();
        
        let file = Path::new("src/main.rs");
        
        tracker.record_edit(file);
        tracker.record_edit(file); // 多次编辑
        
        let record = tracker.get_file_record(file).unwrap();
        assert_eq!(record.edit_count, 2);
        assert!(record.hotness_score > 1.0); // 编辑应该有更高的热度
    }

    #[test]
    fn test_get_hottest_files() {
        let tracker = FileActivityTracker::with_defaults();
        
        let files = vec![
            Path::new("src/a.rs"),
            Path::new("src/b.rs"),
            Path::new("src/c.rs"),
        ];
        
        // 不同频率地访问
        for _ in 0..5 {
            tracker.record_access(&files[0]); // 5 次
        }
        for _ in 0..3 {
            tracker.record_access(&files[1]); // 3 次
        }
        for _ in 0..1 {
            tracker.record_access(&files[2]); // 1 次
        }
        
        let hottest = tracker.get_hottest_files(3);
        
        assert_eq!(hottest.len(), 3);
        assert_eq!(hottest[0].0, Path::new("src/a.rs")); // 应该是最热的
        assert!(hottest[0].1 > hottest[1].1); // a.rs 的热度应该 > b.rs
    }

    #[test]
    fn test_get_relevant_files() {
        let tracker = FileActivityTracker::with_defaults();
        
        let current = Path::new("src/main.rs");
        let related1 = Path::new("src/utils.rs");
        let unrelated = Path::new("vendor/external.rs");
        
        // 模拟共同使用模式: main 和 utils 经常一起出现
        for _ in 0..10 {
            tracker.record_access(current);
            tracker.record_access(related1);
        }
        
        // 偶尔访问无关文件
        tracker.record_access(unrelated);
        
        let relevant = tracker.get_relevant_files(current, 5);
        
        // utils.rs 应该比 external.rs 更相关
        let utils_relevant = relevant.iter()
            .find(|r| r.file_path == related1)
            .expect("utils.rs should be in relevant list");
        
        let external_relevant = relevant.iter()
            .find(|r| r.file_path == unrelated);
        
        assert!(utils_relevant.relevance > 0.3, "utils should be highly relevant");
        
        match external_relevant {
            Some(ext) => assert!(
                utils_relevant.relevance > ext.relevance,
                "utils should be more relevant than external"
            ),
            None => {} // external 可能因相关性太低被过滤掉
        }
    }

    #[test]
    fn test_cleanup_expired() {
        let config = ActivityConfig {
            decay_half_life_secs: 0.001, // 极短的半衰期用于测试
            ..Default::default()
        };
        let tracker = FileActivityTracker::new(config);
        
        let file = Path::new("test.rs");
        tracker.record_access(file);
        
        assert_eq!(tracker.tracked_file_count(), 1);
        
        // 等待一小段时间让记录过期
        thread::sleep(Duration::from_millis(10));
        
        tracker.cleanup_expired();
        
        // 过期的记录应该被清理
        assert_eq!(tracker.tracked_file_count(), 0);
    }

    #[test]
    fn test_stats() {
        let tracker = FileActivityTracker::with_defaults();
        
        tracker.record_access(Path::new("a.rs"));
        tracker.record_edit(Path::new("b.rs"));
        tracker.record_access(Path::new("a.rs"));
        
        let stats = tracker.get_stats();
        
        assert_eq!(stats.tracked_files, 2);
        assert_eq!(stats.total_accesses, 3); // a.rs x2 + b.rs x1 (edit 也算作访问)
        assert_eq!(stats.total_edits, 1);
    }
}
