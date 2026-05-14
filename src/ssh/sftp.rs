use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader, Write};

/// SFTP Protocol Implementation using SSH-based commands
/// Provides secure file transfer with advanced features:
/// - Large file support (>4GB)
/// - Resume/interrupted transfers
/// - Directory operations
/// - File attributes management
/// - Checksum verification

pub struct SftpClient {
    ssh_user: String,
    ssh_host: String,
    ssh_port: u16,
    identity_file: Option<PathBuf>,
    connect_timeout: std::time::Duration,
}

impl SftpClient {
    /// Create new SFTP client instance
    pub fn new(user: &str, host: &str, port: u16) -> Self {
        SftpClient {
            ssh_user: user.to_string(),
            ssh_host: host.to_string(),
            ssh_port: port,
            identity_file: None,
            connect_timeout: std::time::Duration::from_secs(30),
        }
    }

    /// Set identity file for authentication
    pub fn with_identity(mut self, path: &PathBuf) -> Self {
        self.identity_file = Some(path.clone());
        self
    }

    /// Set connection timeout
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    // ==========================================
    // File Operations
    // ==========================================

    /// Upload file to remote server via SFTP (simulated using scp + sftp features)
    pub fn upload(&self, local_path: &Path, remote_path: &Path) -> Result<SftpTransferResult, SftpError> {
        let start = SystemTime::now();
        
        // Verify local file exists
        let metadata = std::fs::metadata(local_path)
            .map_err(|e| SftpError::LocalFileError {
                path: local_path.to_path_buf(),
                message: format!("Failed to stat file: {}", e),
            })?;

        let file_size = metadata.len();
        
        if file_size == 0 {
            return Err(SftpError::LocalFileError {
                path: local_path.to_path_buf(),
                message: "File is empty".to_string(),
            });
        }

        // Use rsync for efficient transfer (supports resume, progress, checksums)
        let result = self._rsync_upload(local_path, remote_path, None)?;
        
        let duration = start.elapsed()
            .map_err(|e| SftpError::InternalError(format!("Time error: {}", e)))?;

        Ok(SftpTransferResult {
            success: result.success,
            bytes_transferred: result.bytes_transferred.unwrap_or(file_size),
            duration,
            speed_bytes_per_sec: if duration.as_secs() > 0 {
                file_size as f64 / duration.as_secs_f64()
            } else { 
                0.0 
            },
            remote_path: remote_path.to_path_buf(),
            checksum: None, // Can be verified separately
            error: result.error,
        })
    }

    /// Download file from remote server
    pub fn download(&self, remote_path: &Path, local_path: &Path) -> Result<SftpTransferResult, SftpError> {
        let start = SystemTime::now();

        // Ensure local directory exists
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SftpError::LocalFileError {
                    path: parent.to_path_buf(),
                    message: format!("Failed to create directory: {}", e),
                })?;
        }

        let result = self._rsync_download(remote_path, local_path, None)?;

        let duration = start.elapsed()
            .map_err(|e| SftpError::InternalError(format!("Time error: {}", e)))?;

        let downloaded_size = std::fs::metadata(local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(SftpTransferResult {
            success: result.success,
            bytes_transferred: downloaded_size,
            duration,
            speed_bytes_per_sec: if duration.as_secs() > 0 {
                downloaded_size as f64 / duration.as_secs_f64()
            } else { 
                0.0 
            },
            remote_path: remote_path.to_path_buf(),
            checksum: None,
            error: result.error,
        })
    }

    /// Upload with resume support (for large files or interrupted transfers)
    pub fn upload_with_resume(
        &self, 
        local_path: &Path, 
        remote_path: &Path,
        offset: Option<u64>  // Offset to resume from
    ) -> Result<SftpTransferResult, SftpError> {
        if let Some(offset) = offset {
            // Use rsync --append-verify for resume capability
            return self._rsync_upload_resume(local_path, remote_path, offset);
        }
        
        // Standard upload if no offset specified
        self.upload(local_path, remote_path)
    }

    /// Upload with progress callback and bandwidth limiting
    pub fn upload_with_progress<F>(
        &self,
        local_path: &Path,
        remote_path: &Path,
        on_progress: F,
        bandwidth_limit: Option<u64>,  // bytes per second
    ) -> Result<SftpTransferResult, SftpError>
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        let file_size = std::fs::metadata(local_path)
            .map_err(|e| SftpError::LocalFileError {
                path: local_path.to_path_buf(),
                message: format!("Failed to get file size: {}", e),
            })?
            .len();

        // Call progress callback initially
        on_progress(0, file_size);

        // Build rsync command with progress tracking
        let result = if let Some(limit) = bandwidth_limit {
            self._rsync_upload_with_bandwidth(local_path, remote_path, limit)?
        } else {
            self._rsync_upload(local_path, remote_path, None)?
        };

        // Call final progress callback
        on_progress(file_size, file_size);

        Ok(result)
    }

    // ==========================================
    // Directory Operations
    // ==========================================

    /// List directory contents with detailed information
    pub fn list_directory(&self, remote_path: &Path) -> Result<Vec<SftpFileInfo>, SftpError> {
        let output = self._execute_ssh_command(&format!(
            "find '{}' -maxdepth 1 -printf '%M %s %T@ %P\\n' 2>/dev/null || ls -la '{}'",
            remote_path.display(),
            remote_path.display()
        ))?;

        if !output.status.success() {
            return Err(SftpError::RemoteOperationFailed {
                operation: "list_directory".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Command failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            });
        }

        let mut files = vec![];
        for line in output.stdout.lines() {
            if let Some(info) = self._parse_ls_line(line.trim()) {
                files.push(info);
            }
        }

        Ok(files)
    }

    /// Create directory recursively (like mkdir -p)
    pub fn create_directory(&self, remote_path: &Path, mode: Option<u32>) -> Result<(), SftpError> {
        let mut cmd = format!("mkdir -p '{}'", remote_path.display());
        
        if let Some(m) = mode {
            cmd = format!("{} && chmod {:o} '{}'", cmd, m, remote_path.display());
        }

        let output = self._execute_ssh_command(&cmd)?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "create_directory".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Failed to create directory: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    /// Remove directory recursively
    pub fn remove_directory(&self, remote_path: &Path, recursive: bool) -> Result<(), SftpError> {
        let cmd = if recursive {
            format!("rm -rf '{}'", remote_path.display())
        } else {
            format!("rmdir '{}'", remote_path.display())
        };

        let output = self._execute_ssh_command(&cmd)?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "remove_directory".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Failed to remove directory: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    /// Rename/move file or directory on remote server
    pub fn rename(&self, old_path: &Path, new_path: &Path) -> Result<(), SftpError> {
        let output = self._execute_ssh_command(&format!(
            "mv '{}' '{}'",
            old_path.display(),
            new_path.display()
        ))?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "rename".to_string(),
                path: old_path.to_path_buf(),
                message: format!("Rename failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    // ==========================================
    // File Attributes Operations
    // ==========================================

    /// Get file/directory metadata
    pub fn stat(&self, remote_path: &Path) -> Result<SftpFileInfo, SftpError> {
        let output = self._execute_ssh_command(&format!(
            "stat '{}' 2>/dev/null || ls -ld '{}'",
            remote_path.display(),
            remote_path.display()
        ))?;

        if !output.status.success() {
            return Err(SftpError::RemoteOperationFailed {
                operation: "stat".to_string(),
                path: remote_path.to_path_buf(),
                message: "File not found".to_string(),
            });
        }

        // Parse stat output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        
        // Try to parse the first line that looks like a stat output
        for line in lines.iter().rev() { // Start from end for ls -ld output
            if line.contains(remote_path.to_str().unwrap_or("")) || 
               line.starts_with('-') || line.starts_with('d') || line.starts_with('l') {
                if let Some(info) = self._parse_ls_line(line.trim()) {
                    return Ok(info);
                }
            }
        }

        Err(SftpError::ParseError {
            message: "Failed to parse file information".to_string(),
        })
    }

    /// Change file permissions
    pub fn chmod(&self, remote_path: &Path, mode: u32) -> Result<(), SftpError> {
        let output = self._execute_ssh_command(&format!(
            "chmod {:o} '{}'",
            mode, remote_path.display()
        ))?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "chmod".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Permission change failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    /// Change file ownership
    pub fn chown(&self, remote_path: &Path, owner: &str, group: Option<&str>) -> Result<(), SftpError> {
        let target = match group {
            Some(g) => format!("{}:{}", owner, g),
            None => owner.to_string(),
        };

        let output = self._execute_ssh_command(&format!(
            "chown {} '{}'",
            target, remote_path.display()
        ))?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "chown".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Ownership change failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    /// Get file checksum (SHA256 by default)
    pub fn checksum(
        &self, 
        remote_path: &Path, 
        algorithm: ChecksumAlgorithm
    ) -> Result<String, SftpError> {
        let cmd = match algorithm {
            ChecksumAlgorithm::Sha256 => format!("sha256sum '{}' | cut -d' ' -f1", remote_path.display()),
            ChecksumAlgorithm::Sha512 => format!("sha512sum '{}' | cut -d' ' -f1", remote_path.display()),
            ChecksumAlgorithm::Md5 => format!("md5sum '{}' | cut -d' ' -f1", remote_path.display()),
        };

        let output = self._execute_ssh_command(&cmd)?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "checksum".to_string(),
                path: remote_path.to_path_buf(),
                message: format!("Checksum calculation failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
    }

    /// Verify file integrity by comparing checksums
    pub fn verify_integrity(
        &self,
        local_path: &Path,
        remote_path: &Path,
        algorithm: ChecksumAlgorithm,
    ) -> Result<bool, SftpError> {
        // Calculate local checksum
        let local_checksum = self._calculate_local_checksum(local_path, algorithm)?;
        
        // Get remote checksum
        let remote_checksum = self.checksum(remote_path, algorithm)?;

        Ok(local_checksum == remote_checksum)
    }

    // ==========================================
    // Symbolic Link Operations
    // ==========================================

    /// Create symbolic link
    pub fn symlink(&self, target: &Path, link_path: &Path) -> Result<(), SftpError> {
        let output = self._execute_ssh_command(&format!(
            "ln -s '{}' '{}'",
            target.display(), link_path.display()
        ))?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "symlink".to_string(),
                path: link_path.to_path_buf(),
                message: format!("Symlink creation failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(())
        }
    }

    /// Read symbolic link target
    pub fn readlink(&self, link_path: &Path) -> Result<PathBuf, SftpError> {
        let output = self._execute_ssh_command(&format!(
            "readlink '{}'",
            link_path.display()
        ))?;

        if !output.status.success() {
            Err(SftpError::RemoteOperationFailed {
                operation: "readlink".to_string(),
                path: link_path.to_path_buf(),
                message: format!("Readlink failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            })
        } else {
            Ok(PathBuf::from(
                String::from_utf8_lossy(&output.stdout).trim()
            ))
        }
    }

    // ==========================================
    // Private Helper Methods
    // ==========================================

    fn _build_ssh_base_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".to_string(), "BatchMode=yes".to_string(),
            "-o".to_string(), format!("ConnectTimeout={}", self.connect_timeout.as_secs()),
        ];

        if let Some(ref identity) = self.identity_file {
            args.push("-i".to_string());
            args.push(identity.display().to_string());
        }

        args.push("-p".to_string());
        args.push(self.ssh_port.to_string());

        args
    }

    fn _execute_ssh_command(&self, command: &str) -> Result<std::process::Output, SftpError> {
        let target = format!("{}@{}", self.ssh_user, self.ssh_host);
        
        let mut cmd = Command::new("ssh");
        for arg in self._build_ssh_base_args() {
            cmd.arg(arg);
        }
        
        cmd.arg(target)
           .arg(command)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        cmd.output().map_err(|e| SftpError::ConnectionError {
            host: self.ssh_host.clone(),
            message: format!("SSH command execution failed: {}", e),
        })
    }

    fn _rsync_upload(
        &self,
        local_path: &Path,
        remote_path: &Path,
        bandwidth_limit: Option<u64>,
    ) -> Result<InternalTransferResult, SftpError> {
        let remote_target = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());
        
        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--progress")
           .arg("--partial")  // Enable resume support
           .arg("-e");

        // Build SSH command string for rsync
        let mut ssh_cmd = format!("ssh -o BatchMode=yes -p {}", self.ssh_port);
        if let Some(ref identity) = self.identity_file {
            ssh_cmd.push_str(&format!(" -i {}", identity.display()));
        }
        cmd.arg(ssh_cmd);

        if let Some(limit) = bandwidth_limit {
            let kb_limit = limit / 1024;
            cmd.arg(format!("--bwlimit={}", kb_limit));
        }

        cmd.arg(local_path.display().to_string())
           .arg(remote_target);

        let output = cmd.output()
            .map_err(|e| SftpError::TransferError {
                message: format!("Rsync upload failed: {}", e),
            })?;

        if output.status.success() {
            let file_size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);
            Ok(InternalTransferResult {
                success: true,
                bytes_transferred: Some(file_size),
                error: None,
            })
        } else {
            Ok(InternalTransferResult {
                success: false,
                bytes_transferred: None,
                error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
            })
        }
    }

    fn _rsync_download(
        &self,
        remote_path: &Path,
        local_path: &Path,
        bandwidth_limit: Option<u64>,
    ) -> Result<InternalTransferResult, SftpError> {
        let remote_source = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());

        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--progress")
           .arg("--partial")
           .arg("-e");

        let mut ssh_cmd = format!("ssh -o BatchMode=yes -p {}", self.ssh_port);
        if let Some(ref identity) = self.identity_file {
            ssh_cmd.push_str(&format!(" -i {}", identity.display()));
        }
        cmd.arg(ssh_cmd);

        if let Some(limit) = bandwidth_limit {
            cmd.arg(format!("--bwlimit={}", limit / 1024));
        }

        cmd.arg(remote_source)
           .arg(local_path.display().to_string());

        let output = cmd.output()
            .map_err(|e| SftpError::TransferError {
                message: format!("Rsync download failed: {}", e),
            })?;

        if output.status.success() {
            let downloaded_size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);
            Ok(InternalTransferResult {
                success: true,
                bytes_transferred: Some(downloaded_size),
                error: None,
            })
        } else {
            Ok(InternalTransferResult {
                success: false,
                bytes_transferred: None,
                error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
            })
        }
    }

    fn _rsync_upload_resume(
        &self,
        local_path: &Path,
        remote_path: &Path,
        offset: u64,
    ) -> Result<SftpTransferResult, SftpError> {
        // Use rsync's --append-verify or --partial for resume
        let remote_target = format!("{}@{}:{}", self.ssh_user, self.ssh_host, remote_path.display());
        
        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
           .arg("--append-verify")  // Resume and verify
           .arg("--partial")
           .arg("-e")
           .arg(format!("ssh -o BatchMode=yes -p {}", self.ssh_port));

        if let Some(ref identity) = self.identity_file {
            let mut ssh_arg = format!("ssh -o BatchMode=yes -p {} -i {}", 
                self.ssh_port, identity.display());
            // Replace last -e argument
            cmd.args.pop();
            cmd.arg(ssh_arg);
        }

        cmd.arg(local_path.display().to_string())
           .arg(remote_target);

        let start = SystemTime::now();
        let output = cmd.output()
            .map_err(|e| SftpError::TransferError {
                message: format!("Resume upload failed: {}", e),
            })?;

        let duration = start.elapsed().unwrap_or(std::time::Duration::ZERO);
        let file_size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);

        Ok(SftpTransferResult {
            success: output.status.success(),
            bytes_transferred: file_size,
            duration,
            speed_bytes_per_sec: if duration.as_secs() > 0 { 
                file_size as f64 / duration.as_secs_f64() 
            } else { 
                0.0 
            },
            remote_path: remote_path.to_path_buf(),
            checksum: None,
            error: if output.status.success { 
                None 
            } else { 
                Some(String::from_utf8_lossy(&output.stderr).to_string()) 
            },
        })
    }

    fn _rsync_upload_with_bandwidth<F>(
        &self,
        local_path: &Path,
        remote_path: &Path,
        bandwidth_limit: u64,
    ) -> Result<SftpTransferResult, SftpError> {
        // This is similar to upload but enforces bandwidth limit
        // In real implementation, you'd stream data and throttle here
        self._rsync_upload(local_path, remote_path, Some(bandwidth_limit))?;

        let file_size = std::fs::metadata(local_path).map(|m| m.len()).unwrap_or(0);
        
        Ok(SftpTransferResult {
            success: true,
            bytes_transferred: file_size,
            duration: std::time::Duration::ZERO, // Would be calculated properly
            speed_bytes_per_sec: bandwidth_limit as f64, // Approximate
            remote_path: remote_path.to_path_buf(),
            checksum: None,
            error: None,
        })
    }

    fn _parse_ls_line(&self, line: &str) -> Option<SftpFileInfo> {
        // Parse standard ls -l output: permissions links owner group size date name
        let parts: Vec<&str> = line.split_whitespace().collect();
        
        if parts.len() >= 7 {
            let permissions = parts[0].to_string();
            let size: u64 = parts[4].parse().unwrap_or(0);
            let name = parts[parts.len()-1].to_string();
            
            let file_type = if permissions.starts_with('d') {
                SftpFileType::Directory
            } else if permissions.starts_with('l') {
                SftpFileType::Symlink
            } else if permissions.starts_with('-') {
                SftpFileType::RegularFile
            } else {
                SftpFileType::Other
            };

            Some(SftpFileInfo {
                name,
                path: PathBuf::from(&name), // Would be full path in real implementation
                file_type,
                size,
                permissions: Some(permissions),
                modification_time: None, // Would parse date field
                owner: Some(parts[2].to_string()),
                group: Some(parts[3].to_string()),
            })
        } else {
            None
        }
    }

    fn _calculate_local_checksum(&self, path: &Path, algorithm: ChecksumAlgorithm) -> Result<String, SftpError> {
        use std::process::Stdio;

        let cmd_str = match algorithm {
            ChecksumAlgorithm::Sha256 => format!("sha256sum '{}'", path.display()),
            ChecksumAlgorithm::Sha512 => format!("sha512sum '{}'", path.display()),
            ChecksumAlgorithm::Md5 => format!("md5sum '{}'", path.display()),
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| SftpError::LocalFileError {
                path: path.to_path_buf(),
                message: format!("Checksum calculation failed: {}", e),
            })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.split_whitespace().next().unwrap_or("").to_string())
        } else {
            Err(SftpError::LocalFileError {
                path: path.to_path_buf(),
                message: "Checksum command failed".to_string(),
            })
        }
    }
}

// ==========================================
// Data Structures
// ==========================================

#[derive(Debug, Clone)]
pub struct SftpTransferResult {
    pub success: bool,
    pub bytes_transferred: u64,
    pub duration: std::time::Duration,
    pub speed_bytes_per_sec: f64,
    pub remote_path: PathBuf,
    pub checksum: Option<String>,
    pub error: Option<String>,
}

struct InternalTransferResult {
    success: bool,
    bytes_transferred: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SftpFileInfo {
    pub name: String,
    pub path: PathBuf,
    pub file_type: SftpFileType,
    pub size: u64,
    pub permissions: Option<String>,
    pub modification_time: Option<SystemTime>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SftpFileType {
    RegularFile,
    Directory,
    Symlink,
    Other,
    BlockDevice,
    CharacterDevice,
    NamedPipe,
    Socket,
}

#[derive(Debug, Clone, Copy)]
pub enum ChecksumAlgorithm {
    Sha256,
    Sha512,
    Md5,
}

#[derive(Debug, Clone)]
pub enum SftpError {
    ConnectionError {
        host: String,
        message: String,
    },
    AuthenticationError {
        message: String,
    },
    LocalFileError {
        path: PathBuf,
        message: String,
    },
    RemoteOperationFailed {
        operation: String,
        path: PathBuf,
        message: String,
    },
    TransferError {
        message: String,
    },
    PermissionDenied {
        path: PathBuf,
        operation: String,
    },
    NotFound {
        path: PathBuf,
    },
    AlreadyExists {
        path: PathBuf,
    },
    InvalidPath {
        path: PathBuf,
        reason: String,
    },
    QuotaExceeded {
        quota: u64,
        used: u64,
    },
    Timeout {
        operation: String,
        duration: std::time::Duration,
    },
    InternalError {
        message: String,
    },
    ParseError {
        message: String,
    },
}

impl std::fmt::Display for SftpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SftpError::ConnectionError { host, message } => {
                write!(f, "SFTP connection error to {}: {}", host, message)
            }
            SftpError::AuthenticationError { message } => {
                write!(f, "SFTP authentication error: {}", message)
            }
            SftpError::LocalFileError { path, message } => {
                write!(f, "SFTP local file error [{}]: {}", path.display(), message)
            }
            SftpError::RemoteOperationFailed { operation, path, message } => {
                write!(f, "SFTP remote operation '{}' on [{}] failed: {}", 
                    operation, path.display(), message)
            }
            SftpError::TransferError { message } => {
                write!(f, "SFTP transfer error: {}", message)
            }
            SftpError::PermissionDenied { path, operation } => {
                write!(f, "SFTP permission denied for '{}' during {}", 
                    path.display(), operation)
            }
            SftpError::NotFound { path } => {
                write!(f, "SFTP file not found: {}", path.display())
            }
            SftpError::AlreadyExists { path } => {
                write!(f, "SFTP file already exists: {}", path.display())
            }
            SftpError::InvalidPath { path, reason } => {
                write!(f, "SFTP invalid path [{}]: {}", path.display(), reason)
            }
            SftpError::QuotaExceeded { quota, used } => {
                write!(f, "SFTP quota exceeded: {}/{}", used, quota)
            }
            SftpError::Timeout { operation, duration } => {
                write!(f, "SFTP timeout after {:?} during {}", duration, operation)
            }
            SftpError::InternalError { message } => {
                write!(f, "SFTP internal error: {}", message)
            }
            SftpError::ParseError { message } => {
                write!(f, "SFTP parse error: {}", message)
            }
        }
    }
}

impl std::error::Error for SftpError {}

/// High-level SFTP session manager with connection pooling
pub struct SftpSessionManager {
    clients: std::collections::HashMap<String, SftpClient>,
    default_config: Option<(String, String, u16)>,  // (user, host, port)
}

impl SftpSessionManager {
    pub fn new() -> Self {
        SftpSessionManager {
            clients: std::collections::HashMap::new(),
            default_config: None,
        }
    }

    pub fn with_default_config(user: &str, host: &str, port: u16) -> Self {
        SftpSessionManager {
            clients: std::collections::HashMap::new(),
            default_config: Some((user.to_string(), host.to_string(), port)),
        }
    }

    /// Get or create SFTP client for a session
    pub fn get_client(&mut self, session_id: &str) -> &mut SftpClient {
        if let Some((user, host, port)) = &self.default_config {
            self.clients.entry(session_id.to_string())
                .or_insert_with(|| SftpClient::new(user, host, *port))
        } else {
            self.clients.entry(session_id.to_string())
                .or_insert_with(|| SftpClient::new("user", "localhost", 22))
        }
    }

    /// Close specific session
    pub fn close_session(&mut self, session_id: &str) {
        self.clients.remove(session_id);
    }

    /// Close all sessions
    pub fn close_all(&mut self) {
        self.clients.clear();
    }

    /// Get active session count
    pub fn active_sessions(&self) -> usize {
        self.clients.len()
    }
}
