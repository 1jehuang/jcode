use std::sync::LazyLock;
use std::any::type_name;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

pub mod error;
pub mod migration_guide;
pub mod mvcc;
pub use error::LockManagerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LockId(pub usize);

#[derive(Debug)]
pub struct LockMetadata {
    pub lock_id: LockId,
    pub name: String,
    pub type_name: String,
    pub created_at: std::time::Instant,
    pub read_count: AtomicUsize,
    pub write_count: AtomicUsize,
    pub contention_count: AtomicUsize,
}

impl Clone for LockMetadata {
    fn clone(&self) -> Self {
        Self {
            lock_id: self.lock_id,
            name: self.name.clone(),
            type_name: self.type_name.clone(),
            created_at: self.created_at,
            read_count: AtomicUsize::new(self.read_count.load(Ordering::Relaxed)),
            write_count: AtomicUsize::new(self.write_count.load(Ordering::Relaxed)),
            contention_count: AtomicUsize::new(self.contention_count.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Debug)]
pub struct LockStats {
    pub lock_id: LockId,
    pub name: String,
    pub type_name: String,
    pub read_count: usize,
    pub write_count: usize,
    pub contention_count: usize,
    pub age_ms: u128,
}

#[derive(Debug)]
pub struct LockSnapshot {
    pub stats: Vec<LockStats>,
    pub total_locks: usize,
    pub total_reads: usize,
    pub total_writes: usize,
    pub total_contention: usize,
    pub timestamp: std::time::Instant,
}

pub struct Shared<T> {
    inner: Arc<RwLock<T>>,
    metadata: Arc<LockMetadata>,
}

impl<T> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared")
            .field("lock_id", &self.metadata.lock_id)
            .field("name", &self.metadata.name)
            .field("type_name", &self.metadata.type_name)
            .finish()
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self::new(T::default(), None)
    }
}

impl<T> Shared<T> {
    pub fn new(value: T, name: Option<&str>) -> Self {
        let type_name = type_name::<T>().to_string();
        let name = name.unwrap_or_else(|| type_name.as_str()).to_string();
        
        let metadata = Arc::new(LockMetadata {
            lock_id: LockManager::register_lock(&name, &type_name),
            name,
            type_name,
            created_at: std::time::Instant::now(),
            read_count: AtomicUsize::new(0),
            write_count: AtomicUsize::new(0),
            contention_count: AtomicUsize::new(0),
        });
        
        Self {
            inner: Arc::new(RwLock::new(value)),
            metadata,
        }
    }

    pub fn with_name(value: T, name: &str) -> Self {
        Self::new(value, Some(name))
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, T> {
        let start = std::time::Instant::now();
        let guard = self.inner.read().await;
        let elapsed = start.elapsed();
        
        self.metadata.read_count.fetch_add(1, Ordering::Relaxed);
        
        if elapsed > std::time::Duration::from_millis(10) {
            self.metadata.contention_count.fetch_add(1, Ordering::Relaxed);
            warn!(
                "Lock contention detected: {} (type: {}) waited {:?}",
                self.metadata.name, self.metadata.type_name, elapsed
            );
        }
        
        guard
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, T> {
        let start = std::time::Instant::now();
        let guard = self.inner.write().await;
        let elapsed = start.elapsed();
        
        self.metadata.write_count.fetch_add(1, Ordering::Relaxed);
        
        if elapsed > std::time::Duration::from_millis(10) {
            self.metadata.contention_count.fetch_add(1, Ordering::Relaxed);
            warn!(
                "Lock contention detected: {} (type: {}) waited {:?}",
                self.metadata.name, self.metadata.type_name, elapsed
            );
        }
        
        guard
    }

    pub fn try_read(&self) -> Option<tokio::sync::RwLockReadGuard<'_, T>> {
        match self.inner.try_read() {
            Ok(guard) => {
                self.metadata.read_count.fetch_add(1, Ordering::Relaxed);
                Some(guard)
            }
            Err(_) => None,
        }
    }

    pub fn try_write(&self) -> Option<tokio::sync::RwLockWriteGuard<'_, T>> {
        match self.inner.try_write() {
            Ok(guard) => {
                self.metadata.write_count.fetch_add(1, Ordering::Relaxed);
                Some(guard)
            }
            Err(_) => None,
        }
    }

    pub fn metadata(&self) -> &Arc<LockMetadata> {
        &self.metadata
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            metadata: Arc::clone(&self.metadata),
        }
    }
}

pub struct LockManager {
    locks: RwLock<HashMap<LockId, Arc<LockMetadata>>>,
    next_id: AtomicUsize,
}

static LOCK_MANAGER: LazyLock<LockManager> = LazyLock::new(|| LockManager {
    locks: RwLock::new(HashMap::new()),
    next_id: AtomicUsize::new(1),
});

impl LockManager {
    fn register_lock(name: &str, type_name: &str) -> LockId {
        let id = LockId(LOCK_MANAGER.next_id.fetch_add(1, Ordering::Relaxed));
        let name_clone = name.to_string();
        let type_name_clone = type_name.to_string();
        
        tokio::spawn(async move {
            let mut locks = LOCK_MANAGER.locks.write().await;
            locks.insert(id, Arc::new(LockMetadata {
                lock_id: id,
                name: name_clone.clone(),
                type_name: type_name_clone.clone(),
                created_at: std::time::Instant::now(),
                read_count: AtomicUsize::new(0),
                write_count: AtomicUsize::new(0),
                contention_count: AtomicUsize::new(0),
            }));
            debug!("Registered new lock: id={:?}, name={}, type={}", id, name_clone, type_name_clone);
        });
        
        id
    }

    pub async fn snapshot() -> LockSnapshot {
        let locks = LOCK_MANAGER.locks.read().await;
        let now = std::time::Instant::now();
        
        let mut stats = Vec::new();
        let mut total_reads = 0;
        let mut total_writes = 0;
        let mut total_contention = 0;
        
        for (_, metadata) in locks.iter() {
            let read_count = metadata.read_count.load(Ordering::Relaxed);
            let write_count = metadata.write_count.load(Ordering::Relaxed);
            let contention_count = metadata.contention_count.load(Ordering::Relaxed);
            
            stats.push(LockStats {
                lock_id: metadata.lock_id,
                name: metadata.name.clone(),
                type_name: metadata.type_name.clone(),
                read_count,
                write_count,
                contention_count,
                age_ms: (now - metadata.created_at).as_millis(),
            });
            
            total_reads += read_count;
            total_writes += write_count;
            total_contention += contention_count;
        }
        
        stats.sort_by_key(|s| s.contention_count);
        stats.reverse();
        
        LockSnapshot {
            stats,
            total_locks: locks.len(),
            total_reads,
            total_writes,
            total_contention,
            timestamp: now,
        }
    }

    pub async fn list_locks() -> Vec<Arc<LockMetadata>> {
        let locks = LOCK_MANAGER.locks.read().await;
        locks.values().cloned().collect()
    }

    pub async fn get_lock_by_id(lock_id: LockId) -> Option<Arc<LockMetadata>> {
        let locks = LOCK_MANAGER.locks.read().await;
        locks.get(&lock_id).cloned()
    }

    pub async fn get_locks_by_name(name: &str) -> Vec<Arc<LockMetadata>> {
        let locks = LOCK_MANAGER.locks.read().await;
        locks.values()
            .filter(|m| m.name == name)
            .cloned()
            .collect()
    }

    pub async fn get_locks_by_type(type_name: &str) -> Vec<Arc<LockMetadata>> {
        let locks = LOCK_MANAGER.locks.read().await;
        locks.values()
            .filter(|m| m.type_name == type_name)
            .cloned()
            .collect()
    }

    pub async fn generate_report() -> String {
        let snapshot = Self::snapshot().await;
        let mut report = String::new();
        
        report.push_str("═══════════════════════════════════════════════════════════════════════\n");
        report.push_str("                    JCODE LOCK MANAGER REPORT                           \n");
        report.push_str("═══════════════════════════════════════════════════════════════════════\n\n");
        
        report.push_str(&format!("Report Time: {:?}\n", snapshot.timestamp));
        report.push_str(&format!("Total Locks: {}\n", snapshot.total_locks));
        report.push_str(&format!("Total Reads: {}\n", snapshot.total_reads));
        report.push_str(&format!("Total Writes: {}\n", snapshot.total_writes));
        report.push_str(&format!("Total Contention Events: {}\n\n", snapshot.total_contention));
        
        report.push_str("═══════════════════════════════════════════════════════════════════════\n");
        report.push_str("                    LOCK STATISTICS (by contention)                    \n");
        report.push_str("═══════════════════════════════════════════════════════════════════════\n");
        report.push_str(format!("{:<10} {:<30} {:<40} {:<10} {:<10} {:<15} {:<10}\n",
            "Lock ID", "Name", "Type", "Reads", "Writes", "Contention", "Age (ms)").as_str());
        report.push_str("-----------------------------------------------------------------------\n");
        
        for stat in &snapshot.stats {
            report.push_str(format!("{:<10} {:<30} {:<40} {:<10} {:<10} {:<15} {:<10}\n",
                stat.lock_id.0,
                truncate(&stat.name, 29),
                truncate(&stat.type_name, 39),
                stat.read_count,
                stat.write_count,
                stat.contention_count,
                stat.age_ms).as_str());
        }
        
        report.push_str("\n═══════════════════════════════════════════════════════════════════════\n");
        report.push_str("                          END OF REPORT                                \n");
        report.push_str("═══════════════════════════════════════════════════════════════════════\n");
        
        report
    }

    pub async fn find_high_contention_locks(threshold: usize) -> Vec<LockStats> {
        let snapshot = Self::snapshot().await;
        snapshot.stats
            .into_iter()
            .filter(|s| s.contention_count >= threshold)
            .collect()
    }

    pub async fn find_old_locks(age_threshold: std::time::Duration) -> Vec<LockStats> {
        let snapshot = Self::snapshot().await;
        snapshot.stats
            .into_iter()
            .filter(|s| std::time::Duration::from_millis(s.age_ms as u64) >= age_threshold)
            .collect()
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

pub async fn dump_lock_report() {
    let report = LockManager::generate_report().await;
    println!("{}", report);
}

#[cfg(feature = "deadlock-detection")]
mod deadlock_detection {
    use super::*;
    use tokio::time::{interval, Duration};

    pub struct DeadlockDetector {
        check_interval: Duration,
        warning_threshold: Duration,
    }

    impl DeadlockDetector {
        pub fn new(check_interval: Duration, warning_threshold: Duration) -> Self {
            Self {
                check_interval,
                warning_threshold,
            }
        }

        pub async fn start(self) {
            let mut interval = interval(self.check_interval);
            loop {
                interval.tick().await;
                self.check_for_deadlocks().await;
            }
        }

        async fn check_for_deadlocks(&self) {
            let locks = LockManager::list_locks().await;
            
            for metadata in locks {
                let age_ms = (std::time::Instant::now() - metadata.created_at).as_millis();
                let age = Duration::from_millis(age_ms as u64);
                
                if age > self.warning_threshold {
                    warn!(
                        "Potential deadlock detected: lock={} (id={:?}) has been held for {:?}",
                        metadata.name, metadata.lock_id, age
                    );
                }
            }
        }
    }
}

#[cfg(feature = "deadlock-detection")]
pub use deadlock_detection::DeadlockDetector;

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_shared_basic() {
        let shared = Shared::new(42, Some("test_counter"));
        
        let mut write_guard = shared.write().await;
        *write_guard = 100;
        drop(write_guard);
        
        let read_guard = shared.read().await;
        assert_eq!(*read_guard, 100);
    }

    #[tokio::test]
    async fn test_shared_clone() {
        let shared = Shared::new(String::from("hello"), Some("test_string"));
        let cloned = shared.clone();
        
        *shared.write().await = "world".to_string();
        
        assert_eq!(*cloned.read().await, "world");
    }

    #[tokio::test]
    async fn test_lock_manager_snapshot() {
        let _shared1 = Shared::new(1, Some("test1"));
        let _shared2 = Shared::new("hello", Some("test2"));
        
        sleep(Duration::from_millis(10)).await;
        
        let snapshot = LockManager::snapshot().await;
        assert!(snapshot.total_locks >= 2);
    }

    #[tokio::test]
    async fn test_lock_manager_report() {
        let _shared = Shared::new(42, Some("report_test"));
        sleep(Duration::from_millis(10)).await;
        
        let report = LockManager::generate_report().await;
        assert!(report.contains("report_test"));
    }
}
