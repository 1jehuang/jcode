//! Cgroups v2 resource isolation for CarpAI services
//!
//! Provides fine-grained resource control using Linux cgroups v2:
//! - CPU quota and shares per service
//! - Memory limits with OOM protection
//! - I/O bandwidth throttling
//! - PID limits to prevent fork bombs
//!
//! Requires:
//! - Linux kernel >= 5.8 (full cgroups v2 support)
//! - Running as root or with CAP_SYS_ADMIN
//! - System mounted with unified cgroups hierarchy

use std::path::PathBuf;
use std::fs;
use tracing::{info, warn};

/// Cgroups v2 controller paths
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Resource limits configuration for a service
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Service name (used as cgroup path)
    pub service_name: String,

    // CPU Limits
    /// CPU quota in microseconds (period is 100000us by default)
    /// e.g., 200000 = 2 CPUs worth of time
    pub cpu_quota_us: Option<u64>,
    /// CPU weight (1-10000, default 100)
    pub cpu_weight: Option<u64>,

    // Memory Limits
    /// Maximum memory in bytes
    pub memory_max: Option<u64>,
    /// Memory high watermark (soft limit) in bytes
    pub memory_high: Option<u64>,
    /// Memory low watermark (protection) in bytes
    pub memory_low: Option<u64>,

    // I/O Limits
    /// Maximum read BPS (bytes per second)
    pub io_read_bps: Option<u64>,
    /// Maximum write BPS
    pub io_write_bps: Option<u64>,
    /// Maximum read IOPS
    pub io_read_iops: Option<u64>,
    /// Maximum write IOPS
    pub io_write_iops: Option<u64>,

    // PID Limits
    /// Maximum number of PIDs
    pub pids_max: Option<u64>,
}

impl ResourceLimits {
    /// Default limits for API service (moderate resources)
    pub fn api_service() -> Self {
        Self {
            service_name: "carpai-api".to_string(),
            cpu_quota_us: Some(400000),   // 4 CPUs
            cpu_weight: Some(200),         // Higher priority
            memory_max: Some(8 * 1024 * 1024 * 1024),  // 8GB
            memory_high: Some(7 * 1024 * 1024 * 1024), // 7GB (soft limit)
            memory_low: Some(4 * 1024 * 1024 * 1024),  // 4GB (protected)
            io_read_bps: Some(500 * 1024 * 1024),      // 500MB/s
            io_write_bps: Some(200 * 1024 * 1024),     // 200MB/s
            io_read_iops: None,
            io_write_iops: None,
            pids_max: Some(1000),
        }
    }

    /// Default limits for Agent service (CPU-intensive)
    pub fn agent_service() -> Self {
        Self {
            service_name: "carpai-agent".to_string(),
            cpu_quota_us: Some(800000),   // 8 CPUs
            cpu_weight: Some(150),
            memory_max: Some(16 * 1024 * 1024 * 1024), // 16GB
            memory_high: Some(14 * 1024 * 1024 * 1024),
            memory_low: Some(8 * 1024 * 1024 * 1024),
            io_read_bps: Some(300 * 1024 * 1024),
            io_write_bps: Some(100 * 1024 * 1024),
            io_read_iops: None,
            io_write_iops: None,
            pids_max: Some(2000),
        }
    }

    /// Default limits for Infrastructure service (I/O bound)
    pub fn infra_service() -> Self {
        Self {
            service_name: "carpai-infra".to_string(),
            cpu_quota_us: Some(200000),   // 2 CPUs
            cpu_weight: Some(100),
            memory_max: Some(4 * 1024 * 1024 * 1024),  // 4GB
            memory_high: Some(3 * 1024 * 1024 * 1024),
            memory_low: Some(2 * 1024 * 1024 * 1024),
            io_read_bps: Some(1024 * 1024 * 1024),     // 1GB/s
            io_write_bps: Some(500 * 1024 * 1024),     // 500MB/s
            io_read_iops: Some(10000),
            io_write_iops: Some(5000),
            pids_max: Some(500),
        }
    }

    /// Default limits for Background service (low priority)
    pub fn background_service() -> Self {
        Self {
            service_name: "carpai-background".to_string(),
            cpu_quota_us: Some(100000),   // 1 CPU
            cpu_weight: Some(50),          // Lower priority
            memory_max: Some(2 * 1024 * 1024 * 1024),  // 2GB
            memory_high: Some(1536 * 1024 * 1024),     // 1.5GB
            memory_low: Some(512 * 1024 * 1024),       // 512MB
            io_read_bps: Some(100 * 1024 * 1024),      // 100MB/s
            io_write_bps: Some(50 * 1024 * 1024),      // 50MB/s
            io_read_iops: None,
            io_write_iops: None,
            pids_max: Some(200),
        }
    }
}

/// Cgroups v2 resource isolator
pub struct CgroupIsolator {
    cgroup_root: PathBuf,
}

impl CgroupIsolator {
    /// Create a new cgroup isolator
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let cgroup_root = PathBuf::from(CGROUP_ROOT);

        if !cgroup_root.exists() {
            return Err("Cgroup filesystem not found at /sys/fs/cgroup".into());
        }

        // Check if cgroups v2 is enabled
        if !is_cgroups_v2_enabled() {
            return Err("Cgroups v2 is not enabled. Please boot with 'systemd.unified_cgroup_hierarchy=1'".into());
        }

        info!("Cgroups v2 isolator initialized at {:?}", cgroup_root);
        Ok(Self { cgroup_root })
    }

    /// Create a cgroup with the specified resource limits
    pub fn create_cgroup(&self, limits: &ResourceLimits) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cgroup_path = self.cgroup_root.join(&limits.service_name);

        // Create cgroup directory
        fs::create_dir_all(&cgroup_path)?;
        info!("Created cgroup: {:?}", cgroup_path);

        // Apply CPU limits
        if let Some(quota) = limits.cpu_quota_us {
            self.write_cgroup_file(&cgroup_path, "cpu.max", &format!("{} 100000", quota))?;
        }
        if let Some(weight) = limits.cpu_weight {
            self.write_cgroup_file(&cgroup_path, "cpu.weight", &weight.to_string())?;
        }

        // Apply memory limits
        if let Some(max) = limits.memory_max {
            self.write_cgroup_file(&cgroup_path, "memory.max", &max.to_string())?;
        }
        if let Some(high) = limits.memory_high {
            self.write_cgroup_file(&cgroup_path, "memory.high", &high.to_string())?;
        }
        if let Some(low) = limits.memory_low {
            self.write_cgroup_file(&cgroup_path, "memory.low", &low.to_string())?;
        }

        // Apply I/O limits
        if limits.io_read_bps.is_some() || limits.io_write_bps.is_some() {
            let io_max = format_io_limits(limits.io_read_bps, limits.io_write_bps,
                                         limits.io_read_iops, limits.io_write_iops);
            if !io_max.is_empty() {
                self.write_cgroup_file(&cgroup_path, "io.max", &io_max)?;
            }
        }

        // Apply PID limits
        if let Some(pids) = limits.pids_max {
            self.write_cgroup_file(&cgroup_path, "pids.max", &pids.to_string())?;
        }

        info!("Applied resource limits to cgroup '{}'", limits.service_name);
        Ok(())
    }

    /// Add a process to the cgroup
    pub fn add_process(&self, limits: &ResourceLimits, pid: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cgroup_path = self.cgroup_root.join(&limits.service_name);
        let procs_file = cgroup_path.join("cgroup.procs");

        fs::write(&procs_file, pid.to_string())?;
        info!("Added PID {} to cgroup '{}'", pid, limits.service_name);
        Ok(())
    }

    /// Remove a cgroup
    pub fn remove_cgroup(&self, service_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let cgroup_path = self.cgroup_root.join(service_name);

        if cgroup_path.exists() {
            fs::remove_dir(&cgroup_path)?;
            info!("Removed cgroup: {:?}", cgroup_path);
        }

        Ok(())
    }

    /// Get current resource usage for a cgroup
    pub fn get_usage(&self, service_name: &str) -> Result<CgroupUsage, Box<dyn std::error::Error + Send + Sync>> {
        let cgroup_path = self.cgroup_root.join(service_name);

        let cpu_stat = self.read_cgroup_file(&cgroup_path, "cpu.stat")?;
        let memory_current = self.read_cgroup_file(&cgroup_path, "memory.current")?;
        let memory_stat = self.read_cgroup_file(&cgroup_path, "memory.stat")?;

        Ok(CgroupUsage {
            service_name: service_name.to_string(),
            cpu_stat,
            memory_current_bytes: memory_current.parse().unwrap_or(0),
            memory_stat,
        })
    }

    /// Helper: Write to a cgroup control file
    fn write_cgroup_file(&self, base_path: &PathBuf, filename: &str, value: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let filepath = base_path.join(filename);
        fs::write(&filepath, value)?;
        Ok(())
    }

    /// Helper: Read from a cgroup control file
    fn read_cgroup_file(&self, base_path: &PathBuf, filename: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let filepath = base_path.join(filename);
        let content = fs::read_to_string(&filepath)?;
        Ok(content.trim().to_string())
    }
}

/// Current cgroup resource usage
#[derive(Debug, Clone)]
pub struct CgroupUsage {
    pub service_name: String,
    pub cpu_stat: String,
    pub memory_current_bytes: u64,
    pub memory_stat: String,
}

impl CgroupUsage {
    /// Parse CPU usage from cpu.stat
    pub fn cpu_usage_usec(&self) -> Option<u64> {
        for line in self.cpu_stat.lines() {
            if line.starts_with("usage_usec") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    return value.parse().ok();
                }
            }
        }
        None
    }

    /// Get memory usage in human-readable format
    pub fn memory_usage_human(&self) -> String {
        let bytes = self.memory_current_bytes;
        if bytes >= 1024 * 1024 * 1024 {
            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        } else if bytes >= 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else if bytes >= 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} B", bytes)
        }
    }
}

/// Check if cgroups v2 is enabled on the system
fn is_cgroups_v2_enabled() -> bool {
    // Check for cgroups v2 mount point
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            if line.contains("cgroup2") && line.contains("/sys/fs/cgroup") {
                return true;
            }
        }
    }
    false
}

/// Format I/O limits for io.max file
fn format_io_limits(
    read_bps: Option<u64>,
    write_bps: Option<u64>,
    read_iops: Option<u64>,
    write_iops: Option<u64>,
) -> String {
    // Format: "MAJOR:MINOR rbps=X wbps=X riops=X wiops=X"
    // Use "default" for all devices
    let mut parts = vec!["default".to_string()];

    if let Some(rbps) = read_bps {
        parts.push(format!("rbps={}", rbps));
    }
    if let Some(wbps) = write_bps {
        parts.push(format!("wbps={}", wbps));
    }
    if let Some(riops) = read_iops {
        parts.push(format!("riops={}", riops));
    }
    if let Some(wiops) = write_iops {
        parts.push(format!("wiops={}", wiops));
    }

    if parts.len() > 1 {
        parts.join(" ")
    } else {
        String::new()
    }
}

/// Initialize cgroups for all CarpAI services
pub fn init_cgroups() -> Result<CgroupIsolator, Box<dyn std::error::Error + Send + Sync>> {
    info!("Initializing cgroups v2 for CarpAI services...");

    let isolator = CgroupIsolator::new()?;

    // Create cgroups for each service type
    let configs = vec![
        ResourceLimits::api_service(),
        ResourceLimits::agent_service(),
        ResourceLimits::infra_service(),
        ResourceLimits::background_service(),
    ];

    for config in configs {
        match isolator.create_cgroup(&config) {
            Ok(_) => info!("Created cgroup for {}", config.service_name),
            Err(e) => warn!("Failed to create cgroup for {}: {}", config.service_name, e),
        }
    }

    Ok(isolator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_limits_defaults() {
        let api = ResourceLimits::api_service();
        assert_eq!(api.cpu_quota_us, Some(400000));
        assert_eq!(api.memory_max, Some(8 * 1024 * 1024 * 1024));

        let agent = ResourceLimits::agent_service();
        assert_eq!(agent.cpu_quota_us, Some(800000));
        assert_eq!(agent.memory_max, Some(16 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_format_io_limits() {
        let result = format_io_limits(
            Some(1000), Some(500),
            Some(100), Some(50)
        );
        assert!(result.contains("rbps=1000"));
        assert!(result.contains("wbps=500"));
        assert!(result.contains("riops=100"));
        assert!(result.contains("wiops=50"));
    }
}
