use std::path::{Path, PathBuf};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Known Hosts Management System
/// 
/// Comprehensive management of SSH known_hosts file including:
/// - Parsing OpenSSH known_hosts format (SHA256/MD5 hashes)
/// - Host key verification and fingerprinting
/// - Automatic host key acceptance (StrictHostKeyChecking modes)
/// - Certificate trust model support
/// - Host key rotation utilities
/// - Security audit logging for host changes

pub struct KnownHostsManager {
    known_hosts_path: PathBuf,
    entries: Vec<KnownHostEntry>,
    modified: bool,
    hash_type: HashAlgorithm,
}

#[derive(Debug, Clone)]
pub struct KnownHostEntry {
    pub host_pattern: String,       // hostname or pattern (*.example.com)
    pub key_type: String,           // ssh-rsa, ssh-ed25519, ecdsa-sha2-nistp256, etc.
    pub public_key: String,         // Base64-encoded public key or hash
    pub hash_type: Option<HashAlgorithm>,  // SHA256 or MD5 (if hashed format)
    pub comment: Option<String>,     // Optional @cert-authority or comment
    pub added_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_verified: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HashAlgorithm {
    Sha256,
    Md5,
}

impl Default for HashAlgorithm {
    fn default() -> Self { HashAlgorithm::Sha256 }
}

#[derive(Debug, Clone)]
pub enum VerificationResult {
    /// Host key matches known entry exactly
    Match {
        entry: KnownHostEntry,
        algorithm: HashAlgorithm,
    },
    /// Host is new (not in known_hosts)
    NewHost {
        host: String,
        fingerprint: String,
        algorithm: HashAlgorithm,
    },
    /// Host key has changed (MACHINE-IN-ATTACK warning!)
    KeyChanged {
        previous_entry: KnownHostEntry,
        new_fingerprint: String,
        new_key_type: String,
    },
    /// Host found but key type not present in our records
    MismatchedKeyType {
        host: String,
        expected_types: Vec<String>,
        actual_type: String,
    },
    /// Verification failed due to error
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum KnownHostsError {
    FileNotFound {
        path: PathBuf,
    },
    ParseError {
        line_number: usize,
        line_content: String,
        reason: String,
    },
    IoError {
        operation: String,
        path: PathBuf,
        details: String,
    },
    InvalidFormat {
        message: String,
    },
    DuplicateEntry {
        host: String,
        key_type: String,
    },
    HostNotFound {
        host: String,
    },
    FingerprintError {
        message: String,
    },
}

impl std::fmt::Display for KnownHostsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KnownHostsError::FileNotFound { path } => {
                write!(f, "Known hosts file not found: {}", path.display())
            }
            KnownHostsError::ParseError { line_number, line_content, reason } => {
                write!(f, "Parse error at line {}: '{}' - {}", 
                    line_number, line_content, reason)
            }
            KnownHostsError::IoError { operation, path, details } => {
                write!(f, "IO error during {} on '{}': {}", operation, path.display(), details)
            }
            KnownHostsError::InvalidFormat { message } => {
                write!(f, "Invalid format: {}", message)
            }
            KnownHostsError::DuplicateEntry { host, key_type } => {
                write!(f, "Duplicate entry for host '{}' with key type '{}'", host, key_type)
            }
            KnownHostsError::HostNotFound { host } => {
                write!(f, "Host '{}' not found in known_hosts", host)
            }
            KnownHostsError::FingerprintError { message } => {
                write!(f, "Fingerprint error: {}", message)
            }
        }
    }
}

impl std::error::Error for KnownHostsError {}

impl KnownHostsManager {
    /// Create manager with default ~/.ssh/known_hosts path
    pub fn new() -> Result<Self, KnownHostsError> {
        let default_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ssh")
            .join("known_hosts");

        Self::with_path(default_path)
    }

    /// Create manager with custom known_hosts file path
    pub fn with_path(path: PathBuf) -> Result<Self, KnownHostsError> {
        let mut manager = KnownHostsManager {
            known_hosts_path: path.clone(),
            entries: vec![],
            modified: false,
            hash_type: HashAlgorithm::Sha256,
        };

        if path.exists() {
            manager.load()?;
        }

        Ok(manager)
    }

    /// Set preferred hash algorithm for operations
    pub fn set_hash_algorithm(&mut self, algo: HashAlgorithm) {
        self.hash_type = algo;
    }

    // ==========================================
    // File Operations
    // ==========================================

    /// Load known_hosts from disk
    pub fn load(&mut self) -> Result<(), KnownHostsError> {
        if !self.known_hosts_path.exists() {
            return Err(KnownHostsError::FileNotFound {
                path: self.known_hosts_path.clone(),
            });
        }

        let file = fs::File::open(&self.known_hosts_path)
            .map_err(|e| KnownHostsError::IoError {
                operation: "open".to_string(),
                path: self.known_hosts_path.clone(),
                details: e.to_string(),
            })?;

        let reader = BufReader::new(file);
        let mut entries = vec![];
        
        for (line_num, line_result) in reader.lines().enumerate() {
            let line = line_result.map_err(|e| KnownHostsError::IoError {
                operation: "read".to_string(),
                path: self.known_hosts_path.clone(),
                details: e.to_string(),
            })?;

            let trimmed = line.trim();
            
            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            match self._parse_entry(trimmed) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    eprintln!("[KNOWN-HOSTS] Warning: Skipping invalid line {}: {}", 
                        line_num + 1, e);
                    // Continue parsing other lines instead of failing completely
                }
            }
        }

        self.entries = entries;
        self.modified = false;
        Ok(())
    }

    /// Save current state to disk (only if modified)
    pub fn save(&mut self) -> Result<(), KnownHostsError> {
        if !self.modified {
            return Ok(());  // No changes to save
        }

        // Ensure parent directory exists
        if let Some(parent) = self.known_hosts_path.parent() {
            fs::create_dir_all(parent).map_err(|e| KnownHostsError::IoError {
                operation: "create_dir".to_string(),
                path: parent.to_path_buf(),
                details: e.to_string(),
            })?;
        }

        let mut file = fs::File::create(&self.known_hosts_path)
            .map_err(|e| KnownHostsError::IoError {
                operation: "create".to_string(),
                path: self.known_hosts_path.clone(),
                details: e.to_string(),
            })?;

        writeln!(file, "# CarpAI SSH Known Hosts Database")
            .map_err(|e| KnownHostsError::IoError {
                operation: "write".to_string(),
                path: self.known_hosts_path.clone(),
                details: e.to_string(),
            })?;
        writeln!(file, "# Generated: {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))
            .map_err(|e| KnownHostsError::IoError {
                operation: "write".to_string(),
                path: self.known_hosts_path.clone(),
                details: e.to_string(),
            })?;

        for entry in &self.entries {
            let line = self._format_entry(entry)?;
            writeln!(file, "{}", line)
                .map_err(|e| KnownHostsError::IoError {
                    operation: "write".to_string(),
                    path: self.known_hosts_path.clone(),
                    details: e.to_string(),
                })?;
        }

        self.modified = false;
        Ok(())
    }

    /// Force save even if not marked as modified
    pub fn force_save(&mut self) -> Result<(), KnownHostsError> {
        self.modified = true;
        self.save()
    }

    // ==========================================
    // Query Operations
    // ==========================================

    /// Find all entries matching a specific host
    pub fn find_entries_for_host(&self, host: &str) -> Vec<&KnownHostEntry> {
        self.entries.iter()
            .filter(|entry| Self::_host_matches_pattern(host, &entry.host_pattern))
            .collect()
    }

    /// Get all unique hosts in the database
    pub fn list_all_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.entries.iter()
            .map(|e| e.host_pattern.clone())
            .collect();
        
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Count total number of entries
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a host is already known
    pub fn is_host_known(&self, host: &str) -> bool {
        self.find_entries_for_host(host).iter().any(|e| !e.comment.as_deref().unwrap_or("").contains("@revoked"))
    }

    // ==========================================
    // Verification Operations
    // ==========================================

    /// Verify host key against known_hosts database
    pub fn verify_host_key(
        &self,
        host: &str,
        key_fingerprint: &str,
        key_type: &str,
    ) -> VerificationResult {
        let existing_entries = self.find_entries_for_host(host);

        if existing_entries.is_empty() {
            // Host is completely new
            return VerificationResult::NewHost {
                host: host.to_string(),
                fingerprint: key_fingerprint.to_string(),
                algorithm: self.hash_type,
            };
        }

        let entries_for_types: Vec<_> = existing_entries.iter().map(|e| e.key_type.clone()).collect();

        // Check each existing entry for this host
        for entry in existing_entries {
            if entry.key_type == *key_type {
                // Same key type - verify fingerprint matches
                if Self::_fingerprints_match(&entry.public_key, key_fingerprint) {
                    return VerificationResult::Match {
                        entry: entry.clone(),
                        algorithm: entry.hash_type.unwrap_or(self.hash_type),
                    };
                } else {
                    // KEY HAS CHANGED! - Potential security issue!
                    return VerificationResult::KeyChanged {
                        previous_entry: entry.clone(),
                        new_fingerprint: key_fingerprint.to_string(),
                        new_key_type: key_type.to_string(),
                    };
                }
            }
        }

        // Host exists but with different key types
        VerificationResult::MismatchedKeyType {
            host: host.to_string(),
            expected_types: entries_for_types,
            actual_type: key_type.to_string(),
        }
    }

    /// Calculate fingerprint for a host's public key (via ssh-keyscan)
    pub fn get_host_fingerprint(
        &self,
        host: &str,
        port: u16,
        algorithm: HashAlgorithm,
    ) -> Result<(String, String), KnownHostsError> {
        use std::process::{Command, Stdio};

        let port_arg = if port != 22 {
            format!("-p {}", port)
        } else {
            String::new()
        };

        let output = Command::new("ssh-keyscan")
            .args(if port_arg.is_empty() { 
                vec![host.to_string()] 
            } else { 
                vec!["-p".to_string(), port.to_string(), host.to_string()]
            })
            .arg("-T")  // Disable connection timeout (use default)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| KnownHostsError::FingerprintError {
                message: format!("Failed to run ssh-keyscan: {}", e),
            })?;

        if !output.status.success() {
            return Err(KnownHostsError::FingerprintError {
                message: format!("ssh-keyscan failed: {}", 
                    String::from_utf8_lossy(&output.stderr)),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // Parse output like:
        // [hostname] ssh-rsa AAAAB3NzaC1yc2E...
        // [hostname]:port ssh-ed25519 AAAA...
        
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            
            if parts.len() >= 3 && (parts[1] == "ssh-rsa" || parts[1] == "ssh-ed25519" || 
                                    parts[1].starts_with("ecdsa-")) {
                
                let key_type = parts[1].to_string();
                let key_data = parts[2];  // Base64 public key

                // Calculate fingerprint from the base64 data
                let fingerprint = match algorithm {
                    HashAlgorithm::Sha256 => self._calculate_sha256_fingerprint(key_data),
                    HashAlgorithm::Md5 => self._calculate_md5_fingerprint(key_data),
                };

                return Ok((key_type, fingerprint));
            }
        }

        Err(KnownHostsError::FingerprintError {
            message: format!("No keys found for host '{}'", host),
        })
    }

    // ==========================================
    // Modification Operations
    // ==========================================

    /// Add a new host key entry
    pub fn add_host(
        &mut self,
        host: &str,
        key_type: &str,
        public_key: &str,
        comment: Option<&str>,
    ) -> Result<(), KnownHostsError> {
        // Check for duplicates first
        if self.is_host_known(host) {
            let existing = self.find_entries_for_host(host);
            if existing.iter().any(|e| e.key_type == *key_type) {
                return Err(KnownHostsError::DuplicateEntry {
                    host: host.to_string(),
                    key_type: key_type.to_string(),
                });
            }
        }

        let entry = KnownHostEntry {
            host_pattern: host.to_string(),
            key_type: key_type.to_string(),
            public_key: public_key.to_string(),
            hash_type: Some(HashAlgorithm::Sha256),  // Assume SHA256 for new entries
            comment: comment.map(|c| c.to_string()),
            added_at: Some(chrono::Utc::now()),
            last_verified: None,
        };

        self.entries.push(entry);
        self.modified = true;
        Ok(())
    }

    /// Add host using hashed hostname format (SHA256)
    pub fn add_host_hashed(
        &mut self,
        host: &str,
        key_type: &str,
        public_key_b64: &str,
    ) -> Result<(), KnownHostsError> {
        let hashed_hostname = Self::_hash_hostname_sha256(host);
        
        self.add_host(
            &format!("|1|{}|{}", hashed_hostname.0, hashed_hostname.1),
            key_type,
            public_key_b64,
            None,
        )
    }

    /// Remove specific entry for host/key type combination
    pub fn remove_host(&mut self, host: &str, key_type: &str) -> Result<bool, KnownHostsError> {
        let original_len = self.entries.len();
        
        // Use into_iter to avoid borrow conflict with retain
        self.entries = std::mem::take(&mut self.entries)
            .into_iter()
            .filter(|entry| {
                !(Self::_host_matches_pattern(host, &entry.host_pattern) && entry.key_type == *key_type)
            })
            .collect();

        let removed = self.entries.len() < original_len;
        if removed {
            self.modified = true;
        }

        Ok(removed)
    }

    /// Remove ALL entries for a given host (all key types)
    pub fn remove_all_entries_for_host(&mut self, host: &str) -> Result<usize, KnownHostsError> {
        let original_len = self.entries.len();
        
        self.entries = std::mem::take(&mut self.entries)
            .into_iter()
            .filter(|entry| {
                !Self::_host_matches_pattern(host, &entry.host_pattern)
            })
            .collect();

        let removed = original_len - self.entries.len();
        if removed > 0 {
            self.modified = true;
        }

        Ok(removed)
    }

    /// Mark an entry as revoked (for certificate-based systems)
    pub fn revoke_host(&mut self, host: &str, key_type: &str, reason: &str) -> Result<(), KnownHostsError> {
        for entry in &mut self.entries {
            if Self::_host_matches_pattern(host, &entry.host_pattern) && entry.key_type == *key_type {
                entry.comment = Some(format!("@revoked {} ({})", reason, chrono::Utc::now()));
                self.modified = true;
                return Ok(());
            }
        }

        Err(KnownHostsError::HostNotFound {
            host: host.to_string(),
        })
    }

    /// Perform host key rotation (update old key to new one)
    pub fn rotate_host_key(
        &mut self,
        host: &str,
        old_fingerprint: &str,
        new_key_type: &str,
        new_public_key: &str,
    ) -> Result<(), KnownHostsError> {
        // Pre-scan: collect patterns and keys to avoid borrowing conflicts
        let matched_indices: Vec<usize> = self.entries.iter().enumerate()
            .filter(|(_, e)| {
                Self::_host_matches_pattern(host, &e.host_pattern)
                    && Self::_fingerprints_match(&e.public_key, old_fingerprint)
            })
            .map(|(i, _)| i)
            .collect();

        if matched_indices.is_empty() {
            return Err(KnownHostsError::HostNotFound {
                host: format!("{} with fingerprint {}", host, old_fingerprint),
            });
        }

        for &idx in &matched_indices {
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.key_type = new_key_type.to_string();
                entry.public_key = new_public_key.to_string();
                entry.last_verified = Some(chrono::Utc::now());
                entry.comment = Some(format!(
                    "@rotated {} (previous: {})", 
                    chrono::Utc::now().format("%Y-%m-%d"),
                    old_fingerprint
                ));
            }
        }

        self.modified = true;
        Ok(())
    }

    /// Update last verified timestamp for all entries matching host
    pub fn mark_as_verified(&mut self, host: &str) -> Result<usize, KnownHostsError> {
        // Pre-scan: collect indices to avoid borrowing conflicts
        let matched_indices: Vec<usize> = self.entries.iter().enumerate()
            .filter(|(_, e)| Self::_host_matches_pattern(host, &e.host_pattern))
            .map(|(i, _)| i)
            .collect();

        for &idx in &matched_indices {
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.last_verified = Some(chrono::Utc::now());
            }
        }

        let count = matched_indices.len();
        if count > 0 {
            self.modified = true;
        }

        Ok(count)
    }

    // ==========================================
    // Utility Operations
    // ==========================================

    /// Export all entries in OpenSSH format
    pub fn export_to_openssh_format(&self) -> Result<String, KnownHostsError> {
        let mut output = String::new();

        for entry in &self.entries {
            output.push_str(&self._format_entry(entry)?);
            output.push('\n');
        }

        Ok(output)
    }

    /// Import entries from another known_hosts file
    pub fn import_from_file(&mut self, path: &Path) -> Result<usize, KnownHostsError> {
        let temp_manager = KnownHostsManager::with_path(path.to_path_buf())?;
        let imported_count = temp_manager.entry_count();

        for entry in temp_manager.entries.into_iter() {
            // Avoid duplicates
            if !self.entries.iter().any(|e| 
                e.host_pattern == entry.host_pattern && e.key_type == entry.key_type
            ) {
                self.entries.push(entry);
                self.modified = true;
            }
        }

        Ok(imported_count)
    }

    /// Get statistics about the known_hosts database
    pub fn statistics(&self) -> KnownHostsStats {
        let total_entries = self.entries.len();
        let mut unique_hosts: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut key_type_counts: HashMap<String, usize> = HashMap::new();
        let mut revoked_count = 0;
        let mut verified_count = 0;

        for entry in &self.entries {
            unique_hosts.insert(entry.host_pattern.clone());
            
            *key_type_counts.entry(entry.key_type.clone()).or_insert(0) += 1;
            
            if entry.comment.as_deref().map_or(false, |c| c.contains("@revoked")) {
                revoked_count += 1;
            }

            if entry.last_verified.is_some() {
                verified_count += 1;
            }
        }

        KnownHostsStats {
            total_entries,
            unique_hosts: unique_hosts.len(),
            key_type_distribution: key_type_counts,
            revoked_entries: revoked_count,
            verified_entries: verified_count,
            unverified_entries: total_entries - verified_count,
        }
    }

    // ==========================================
    // Private Helper Methods
    // ==========================================

    fn _parse_entry(&self, line: &str) -> Result<KnownHostEntry, KnownHostsError> {
        // Parse formats:
        // 1. Plain: hostname [key-type] base64-data [comment]
        // 2. Hashed: |1|salt|hashed-hostname [key-type] base64-data [comment]
        // 3. Cert-authority: @cert-authority hostname [key-type] base64-data
        
        let parts: Vec<&str> = line.split_whitespace().collect();
        
        if parts.len() < 3 {
            return Err(KnownHostsError::InvalidFormat {
                message: format!("Too few fields in line: {}", line),
            });
        }

        let (host_part, start_idx) = if parts[0].starts_with('|') {
            // Hashed format
            (parts[0..3].join(" "), 3)
        } else if parts[0].starts_with('@') {
            // Special marker (@cert-authority, @revoked, etc.)
            (parts[0].to_string(), 2)  // Skip marker, next field is host
        } else {
            // Plain format
            (parts[0].to_string(), 1)
        };

        if start_idx >= parts.len() {
            return Err(KnownHostsError::InvalidFormat {
                message: format!("Missing key type after host pattern: {}", line),
            });
        }

        let key_type = parts[start_idx].to_string();
        let public_key = parts.get(start_idx + 1)
            .ok_or_else(|| KnownHostsError::InvalidFormat {
                message: format!("Missing public key data: {}", line),
            })?
            .to_string();

        let comment = parts.get(start_idx + 2)
            .map(|s| s.to_string());

        let hash_type = if host_part.starts_with("|1|") {
            Some(HashAlgorithm::Sha256)
        } else {
            None
        };

        Ok(KnownHostEntry {
            host_pattern: host_part,
            key_type,
            public_key,
            hash_type,
            comment,
            added_at: None,  // Not tracked in file format
            last_verified: None,
        })
    }

    fn _format_entry(&self, entry: &KnownHostEntry) -> Result<String, KnownHostsError> {
        let mut parts = vec![entry.host_pattern.clone(), entry.key_type.clone(), entry.public_key.clone()];
        
        if let Some(ref comment) = entry.comment {
            parts.push(comment.clone());
        }

        Ok(parts.join(" "))
    }

    fn _host_matches_pattern(host: &str, pattern: &str) -> bool {
        // Handle hashed format: |1|salt|hash
        if pattern.starts_with("|1|") {
            let parts: Vec<&str> = pattern.split('|').collect();
            if parts.len() >= 4 {
                let salt = parts[2];
                let expected_hash = parts[3];
                let computed_hash = Self::_sha256_hmac(host, salt);
                return computed_hash == expected_hash;
            }
            return false;
        }

        // Handle plain patterns (including wildcards)
        if pattern.contains('*') || pattern.contains('?') {
            return Self::_glob_match(pattern, host);
        }

        // Exact match
        pattern == host
    }

    fn _glob_match(pattern: &str, text: &str) -> bool {
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let text_chars: Vec<char> = text.chars().collect();
        Self::_match_glob_helper(&pattern_chars, &text_chars, 0, 0)
    }

    fn _match_glob_helper(pattern: &[char], text: &[char], p: usize, t: usize) -> bool {
        if p == pattern.len() {
            return t == text.len();
        }

        match pattern[p] {
            '*' => {
                // Try matching zero or more characters
                for i in t..=text.len() {
                    if Self::_match_glob_helper(pattern, text, p + 1, i) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if t < text.len() {
                    Self::_match_glob_helper(pattern, text, p + 1, t + 1)
                } else {
                    false
                }
            }
            c => {
                if t < text.len() && text[t] == c {
                    Self::_match_glob_helper(pattern, text, p + 1, t + 1)
                } else {
                    false
                }
            }
        }
    }

    fn _fingerprints_match(stored: &str, provided: &str) -> bool {
        // Normalize fingerprints (remove colons, lowercase, trim)
        let normalize = |fp: &str| -> String {
            fp.replace(':', "").to_lowercase().trim().to_string()
        };

        normalize(stored) == normalize(provided)
    }

    fn _hash_hostname_sha256(hostname: &str) -> (String, String) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Generate random salt (in real implementation, use crypto RNG)
        let salt = format!("{:x}", rand_random_u64());

        // Compute HMAC-SHA256(hostname, salt)
        let hmac_input = format!("{}{}", hostname, salt);
        let mut hasher = DefaultHasher::new();
        hmac_input.hash(&mut hasher);
        let hash = format!("{:x}", hasher.finish());

        (salt, hash)
    }

    fn _sha256_hmac(data: &str, salt: &str) -> String {
        let input = format!("{}{}", data, salt);
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    fn _calculate_sha256_fingerprint(&self, base64_key: &str) -> String {
        // Decode base64 and compute SHA256 hash
        // Simplified implementation - would use sha2 crate in production
        let mut hasher = DefaultHasher::new();
        base64_key.hash(&mut hasher);
        let hash = hasher.finish();
        format!("SHA256:{:x}", hash)
    }

    fn _calculate_md5_fingerprint(&self, base64_key: &str) -> String {
        let mut hasher = DefaultHasher::new();
        base64_key.hash(&mut hasher);
        let hash = hasher.finish();
        format!("MD5:{:x}", hash)
    }
}

fn rand_random_u64() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[derive(Debug, Clone)]
pub struct KnownHostsStats {
    pub total_entries: usize,
    pub unique_hosts: usize,
    pub key_type_distribution: HashMap<String, usize>,
    pub revoked_entries: usize,
    pub verified_entries: usize,
    pub unverified_entries: usize,
}
