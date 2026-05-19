use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};


/// File Transfer Progress Callback
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send + Sync>;

/// File Transfer Result
#[derive(Debug, Clone)]
pub struct TransferResult {
    pub success: bool,
    pub bytes_transferred: u64,
    pub duration: Duration,
    pub speed_bytes_per_sec: f64,
    pub error: Option<String>,
}

/// Enhanced File Transfer with SCP and Rsync support
pub struct FileTransfer {
    ssh_user: String,
    ssh_host: String,
    ssh_port: u16,
    identity_file: Option<PathBuf>,
    use_rsync: bool,
    compression: bool,
    bandwidth_limit: Option<u64>,  // Bytes per second
}

impl FileTransfer {
    /// Create new file transfer instance
    pub fn new(user: &str, host: &str, port: u16) -> Self {
        FileTransfer {
            ssh_user: user.to_string(),
            ssh_host: host.to_string(),
            ssh_port: port,
            identity_file: None,
            use_rsync: Self::_check_rsync_available(),
            compression: true,
            bandwidth_limit: None,
        }
    }

    /// Set identity file for authentication
    pub fn with_identity(mut self, path: &PathBuf) -> Self {
        self.identity_file = Some(path.clone());
        self
    }

    /// Enable/disable rsync (fallback to SCP if not available)
    pub fn use_rsync(mut self, use_it: bool) -> Self {
        self.use_rsync = use_it && Self::_check_rsync_available();
        self
    }

    /// Enable/disable compression
    pub fn compress(mut self, enable: bool) -> Self {
        self.compression = enable;
        self
    }

    /// Set bandwidth limit in bytes per second
    pub fn limit_bandwidth(mut self, bytes_per_sec: u64) -> Self {
        self.bandwidth_limit = Some(bytes_per_sec);
        self
    }

    /// Upload single file to remote host
    pub fn upload_file(&self, local_path: &Path, remote_path: &Path) -> Result<TransferResult, String> {
        let start = Instant::now();
        
        let result = if self.use_rsync {
            self._rsync_upload(local_path, remote_path)
        } else {
            self._scp_upload(local_path, remote_path)
        };

        let duration = start.elapsed();
        let file_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        match result {
            Ok(()) => Ok(TransferResult {
                success: true,
                bytes_transferred: file_size,
                duration,
                speed_bytes_per_sec: if duration.as_secs() > 0 { 
                    file_size as f64 / duration.as_secs() as f64 
                } else { 0.0 },
                error: None,
            }),
            Err(e) => Ok(TransferResult {
                success: false,
                bytes_transferred: 0,
                duration,
                speed_bytes_per_sec: 0.0,
                error: Some(e),
            }),
        }
    }

    /// Download single file from remote host
    pub fn download_file(&self, remote_path: &Path, local_path: &Path) -> Result<TransferResult, String> {
        let start = Instant::now();

        let result = if self.use_rsync {
            self._rsync_download(remote_path, local_path)
        } else {
            self._scp_download(remote_path, local_path)
        };

        let duration = start.elapsed();
        let file_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        match result {
            Ok(()) => Ok(TransferResult {
                success: true,
                bytes_transferred: file_size,
                duration,
                speed_bytes_per_sec: if duration.as_secs() > 0 { 
                    file_size as f64 / duration.as_secs() as f64 
                } else { 0.0 },
                error: None,
            }),
            Err(e) => Ok(TransferResult {
                success: false,
                bytes_transferred: 0,
                duration,
                speed_bytes_per_sec: 0.0,
                error: Some(e),
            }),
        }
    }

    /// Upload directory recursively
    pub fn upload_directory(&self, local_dir: &Path, remote_dir: &Path, progress: Option<ProgressCallback>) -> Result<Vec<TransferResult>, String> {
        let mut results = vec![];
        let files = self._collect_files(local_dir)?;
        let total_files = files.len();

        for (i, (local_file, relative_path)) in files.iter().enumerate() {
            let remote_file = remote_dir.join(relative_path);
            
            // Create parent directories on remote
            if let Some(parent) = remote_file.parent() {
                self._create_remote_dirs(parent)?;
            }

            // Call progress callback
            if let Some(ref cb) = progress {
                cb(i as u64, total_files as u64);
            }

            match self.upload_file(local_file, &remote_file)? {
                result @ TransferResult { success: true, .. } => results.push(result),
                result @ TransferResult { success: false, .. } => {
                    results.push(result);
                    // Continue with other files even if one fails
                }
            }
        }

        Ok(results)
    }

    /// Download directory recursively
    pub fn download_directory(&self, remote_dir: &Path, local_dir: &Path, progress: Option<ProgressCallback>) -> Result<Vec<TransferResult>, String> {
        let mut results = vec![];

        // Create local directory
        std::fs::create_dir_all(local_dir)
            .map_err(|e| format!("Failed to create local directory: {}", e))?;

        // List remote directory contents
        let remote_files = self._list_remote_directory(remote_dir)?;
        let total_files = remote_files.len();

        for (i, remote_file) in remote_files.iter().enumerate() {
            let (full_path, relative_path) = remote_file;
            let local_file = local_dir.join(relative_path);

            // Create parent directories locally
            if let Some(parent) = local_file.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create local directory: {}", e))?;
            }

            // Call progress callback
            if let Some(ref cb) = progress {
                cb(i as u64, total_files as u64);
            }

            match self.download_file(full_path, &local_file)? {
                result @ TransferResult { success: true, .. } => results.push(result),
                result @ TransferResult { success: false, .. } => {
                    results.push(result);
                }
            }
        }

        Ok(results)
    }

    /// Synchronize local directory to remote (like rsync --delete)
    pub fn sync_to_remote(&self, local_dir: &Path, remote_dir: &Path) -> Result<SyncResult, String> {
        if !self.use_rsync {
            return Err("Sync requires rsync. Use upload_directory instead.".to_string());
        }

        let start = Instant::now();
        let remote_target = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_dir.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--delete")
           .arg("--progress");

        if self.compression {
            cmd.arg("--compress");
        }

        if let Some(limit) = self.bandwidth_limit {
            cmd.arg(format!("--bwlimit={}", limit / 1024)); // rsync uses KB/s
        }

        if let Some(ref identity) = self.identity_file {
            cmd.arg("-e").arg(format!("ssh -i {} -p {}", identity.display(), self.ssh_port));
        } else {
            cmd.arg("-e").arg(format!("ssh -p {}", self.ssh_port));
        }

        cmd.arg(local_dir.display().to_string())
           .arg(remote_target);

        let output = cmd.output()
            .map_err(|e| format!("Rsync sync failed: {}", e))?;

        let duration = start.elapsed();

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            
            // Parse rsync output for statistics
            let stats = self._parse_rsync_stats(&stdout);

            Ok(SyncResult {
                success: true,
                files_transferred: stats.files_transferred,
                bytes_transferred: stats.bytes_sent + stats.bytes_received,
                duration,
                error: None,
                details: Some(stdout.to_string()),
            })
        } else {
            Ok(SyncResult {
                success: false,
                files_transferred: 0,
                bytes_transferred: 0,
                duration,
                error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
                details: None,
            })
        }
    }

    /// Synchronize remote directory to local
    pub fn sync_from_remote(&self, remote_dir: &Path, local_dir: &Path) -> Result<SyncResult, String> {
        if !self.use_rsync {
            return Err("Sync requires rsync. Use download_directory instead.".to_string());
        }

        let start = Instant::now();
        let remote_source = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_dir.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--delete")
           .arg("--progress")
           .arg("-e")
           .arg(if let Some(ref identity) = self.identity_file {
               format!("ssh -i {} -p {}", identity.display(), self.ssh_port)
           } else {
               format!("ssh -p {}", self.ssh_port)
           })
           .arg(remote_source)
           .arg(local_dir.display().to_string());

        let output = cmd.output()
            .map_err(|e| format!("Rsync sync failed: {}", e))?;

        let duration = start.elapsed();

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stats = self._parse_rsync_stats(&stdout);

            Ok(SyncResult {
                success: true,
                files_transferred: stats.files_transferred,
                bytes_transferred: stats.bytes_sent + stats.bytes_received,
                duration,
                error: None,
                details: Some(stdout.to_string()),
            })
        } else {
            Ok(SyncResult {
                success: false,
                files_transferred: 0,
                bytes_transferred: 0,
                duration,
                error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
                details: None,
            })
        }
    }

    // Private helper methods

    fn _scp_upload(&self, local_path: &Path, remote_path: &Path) -> Result<(), String> {
        let remote_dest = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P").arg(self.ssh_port.to_string())
           .arg("-o").arg("BatchMode=yes");

        if self.compression {
            cmd.arg("-C");
        }

        if let Some(ref identity) = self.identity_file {
            cmd.arg("-i").arg(identity.display().to_string());
        }

        cmd.arg(local_path.display().to_string())
           .arg(remote_dest)
           .status()
           .map_err(|e| format!("SCP upload failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP upload failed".to_string()) }
           })
    }

    fn _scp_download(&self, remote_path: &Path, local_path: &Path) -> Result<(), String> {
        let remote_src = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P").arg(self.ssh_port.to_string())
           .arg("-o").arg("BatchMode=yes");

        if self.compression {
            cmd.arg("-C");
        }

        if let Some(ref identity) = self.identity_file {
            cmd.arg("-i").arg(identity.display().to_string());
        }

        cmd.arg(remote_src)
           .arg(local_path.display().to_string())
           .status()
           .map_err(|e| format!("SCP download failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP download failed".to_string()) }
           })
    }

    fn _rsync_upload(&self, local_path: &Path, remote_path: &Path) -> Result<(), String> {
        let remote_dest = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("-e")
           .arg(if let Some(ref identity) = self.identity_file {
               format!("ssh -i {} -p {}", identity.display(), self.ssh_port)
           } else {
               format!("ssh -p {}", self.ssh_port)
           })
           .arg(local_path.display().to_string())
           .arg(remote_dest);

        let output = cmd.output()
            .map_err(|e| format!("Rsync upload failed: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Rsync error: {}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    fn _rsync_download(&self, remote_path: &Path, local_path: &Path) -> Result<(), String> {
        let remote_src = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("-e")
           .arg(if let Some(ref identity) = self.identity_file {
               format!("ssh -i {} -p {}", identity.display(), self.ssh_port)
           } else {
               format!("ssh -p {}", self.ssh_port)
           })
           .arg(remote_src)
           .arg(local_path.display().to_string());

        let output = cmd.output()
            .map_err(|e| format!("Rsync download failed: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Rsync error: {}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    fn _collect_files(&self, dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let mut files = vec![];
        
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Entry error: {}", e))?;
            let path = entry.path();

            if path.is_dir() {
                let name = entry.file_name();
                let sub_files = self._collect_files(&path)?;
                for (file, relative) in sub_files {
                    files.push((file, PathBuf::from(name.clone()).join(relative)));
                }
            } else if path.is_file() {
                let name = entry.file_name();
                files.push((path, PathBuf::from(name)));
            }
        }

        Ok(files)
    }

    fn _list_remote_directory(&self, remote_dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        // Use SSH to list directory contents
        let target = format!("{}@{}", self.ssh_user, self.ssh_host);
        let list_cmd = format!("find {} -type f", remote_dir.display());

        let mut cmd = Command::new("ssh");
        cmd.arg("-o").arg("BatchMode=yes")
           .arg("-p").arg(self.ssh_port.to_string())
           .arg(target)
           .arg(list_cmd)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let output = cmd.output()
            .map_err(|e| format!("Failed to list remote directory: {}", e))?;

        if !output.status.success() {
            return Err(format!("Remote listing failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let base_path = remote_dir;
        let mut files = vec![];

        for line in stdout.lines() {
            let full_path = PathBuf::from(line.trim());
            if let Ok(relative) = full_path.strip_prefix(base_path) {
                files.push((full_path.clone(), relative.to_path_buf()));
            }
        }

        Ok(files)
    }

    fn _create_remote_dirs(&self, remote_path: &Path) -> Result<(), String> {
        let target = format!("{}@{}", self.ssh_user, self.ssh_host);
        let mkdir_cmd = format!("mkdir -p {}", remote_path.display());

        let mut cmd = Command::new("ssh");
        cmd.arg("-o").arg("BatchMode=yes")
           .arg("-p").arg(self.ssh_port.to_string())
           .arg(target)
           .arg(mkdir_cmd)
           .stdout(Stdio::null())
           .stderr(Stdio::piped());

        let output = cmd.output()
            .map_err(|e| format!("Failed to create remote directory: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Remote mkdir failed: {}", String::from_utf8_lossy(&output.stderr)))
        }
    }

    fn _check_rsync_available() -> bool {
        which::which("rsync").is_ok()
    }

    fn _parse_rsync_stats(&self, output: &str) -> RsyncStats {
        let mut stats = RsyncStats::default();

        for line in output.lines() {
            if line.contains("Number of files:") {
                if let Some(num_str) = line.split(':').nth(1) {
                    stats.total_files = num_str.trim().parse().unwrap_or(0);
                }
            } else if line.contains("Number of files transferred:") {
                if let Some(num_str) = line.split(':').nth(1) {
                    stats.files_transferred = num_str.trim().parse().unwrap_or(0);
                }
            } else if line.contains("Total file size:") {
                if let Some(size_str) = line.split(':').nth(1) {
                    stats.total_size = Self::_parse_size(size_str.trim());
                }
            } else if line.contains("Total transferred file size:") {
                if let Some(size_str) = line.split(':').nth(1) {
                    stats.transferred_size = Self::_parse_size(size_str.trim());
                }
            } else if line.contains("sent ") && line.contains("bytes") {
                if let Some(bytes_str) = line.split(' ').next() {
                    stats.bytes_sent = Self::_parse_size(bytes_str);
                }
            } else if line.contains("received ") && line.contains("bytes") {
                if let Some(bytes_str) = line.split(' ').next() {
                    stats.bytes_received = Self::_parse_size(bytes_str);
                }
            }
        }

        stats
    }

    fn _parse_size(size_str: &str) -> u64 {
        let size_str = size_str.trim().trim_end_matches(',');
        let num_part: String = size_str.chars().take_while(|c| (*c).is_digit(10) || *c == '.').collect();
        let unit_part: String = size_str.chars().skip_while(|c| (*c).is_digit(10) || *c == '.').collect();

        let num: f64 = num_part.parse().unwrap_or(0.0);

        match unit_part.to_lowercase().as_str() {
            "k" | "kb" => (num * 1024.0) as u64,
            "m" | "mb" => (num * 1024.0 * 1024.0) as u64,
            "g" | "gb" => (num * 1024.0 * 1024.0 * 1024.0) as u64,
            "t" | "tb" => (num * 1024.0 * 1024.0 * 1024.0 * 1024.0) as u64,
            _ => num as u64,
        }
    }
}

#[derive(Debug, Default)]
struct RsyncStats {
    total_files: usize,
    files_transferred: usize,
    total_size: u64,
    transferred_size: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

/// Synchronization Result
#[derive(Debug, Clone)]
pub struct SyncResult {
    pub success: bool,
    pub files_transferred: usize,
    pub bytes_transferred: u64,
    pub duration: Duration,
    pub error: Option<String>,
    pub details: Option<String>,
}
