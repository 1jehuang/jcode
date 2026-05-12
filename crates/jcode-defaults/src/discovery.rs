//! Environment Discovery and Auto-Detection
//!
//! Automatically detects system capabilities, installed software,
//! and optimal configuration for zero-config startup.

use std::path::{Path, PathBuf};
use std::process::Command;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use tracing::{info, debug, warn};

/// Detected system capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemCapabilities {
    /// Operating system information
    pub os: OsInfo,
    
    /// CPU information
    pub cpu: CpuInfo,
    
    /// Memory information
    pub memory: MemoryInfo,
    
    /// GPU information (if available)
    pub gpu: Option<GpuInfo>,
    
    /// Available LLM providers (auto-detected)
    pub available_providers: Vec<DetectedProvider>,
    
    /// VS Code installation info
    pub vscode: Option<VscodeInstallation>,
    
    /// Network capabilities
    pub network: NetworkCapabilities,
}

/// Operating System Information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub family: String, // windows, macos, linux
    pub architecture: String, // x86_64, arm64
}

/// CPU Information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub model_name: String,
    pub cores: usize,
    pub logical_processors: usize,
    pub max_frequency_mhz: Option<f64>,
}

/// Memory Information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_gb: f64,
    pub available_gb: f64,
    pub swap_gb: f64,
}

/// GPU Information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_gb: f64,
    pub driver_version: String,
    pub cuda_available: bool,
    pub vulkan_available: bool,
}

/// Detected LLM Provider with availability status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedProvider {
    pub provider_type: String,
    pub display_name: String,
    pub is_configured: bool,
    pub api_key_source: Option<String>, // "env:DEEPSEEK_API_KEY", "config file", etc.
    pub endpoint_url: Option<String>,
    pub recommended_models: Vec<String>,
    pub priority: u8, // 0 = highest priority (auto-select this first)
}

/// VS Code Installation Details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeInstallation {
    pub path: PathBuf,
    pub version: String,
    pub channel: String, // stable, insider, exploration
    pub extensions_dir: PathBuf,
    pub user_data_dir: PathBuf,
    pub is_running: bool,
}

/// Network Capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkCapabilities {
    pub has_internet: bool,
    pub can_reach_openai: bool,
    pub can_reach_deepseek: bool,
    pub proxy_detected: bool,
    pub proxy_url: Option<String>,
}

impl EnvironmentDetector {
    /// Detect all system capabilities and available providers
    pub async fn detect_all() -> Result<SystemCapabilities> {
        let mut caps = SystemCapabilities {
            os: Self::detect_os()?,
            cpu: Self::detect_cpu()?,
            memory: Self::detect_memory()?,
            gpu: Self::detect_gpu().ok(),
            available_providers: vec![],
            vscode: Self::detect_vscode().ok(),
            network: Self::detect_network().await?,
        };
        
        // Detect available LLM providers
        caps.available_providers = Self::detect_llm_providers(&caps).await?;
        
        Ok(caps)
    }
    
    /// Detect operating system information
    fn detect_os() -> Result<OsInfo> {
        let info = sys_info::os_type();
        let version = sys_info::os_release();
        
        #[cfg(target_os = "windows")]
        let family = "windows".to_string();
        #[cfg(target_os = "macos")]
        let family = "macos".to_string();
        #[cfg(target_os = "linux")]
        let family = "linux".to_string();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let family = "unknown".to_string();
        
        // Detect architecture
        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else if cfg!(target_arch = "x86") {
            "x86"
        } else {
            "unknown"
        }.to_string();
        
        Ok(OsInfo {
            name: format!("{:?}", info),
            version,
            family,
            architecture: arch,
        })
    }
    
    /// Detect CPU information
    fn detect_cpu() -> Result<CpuInfo> {
        use sys_info;
        
        let cpu_info = cpu_info()?;
        let num_cpus = num_cpus::get();
        
        Ok(CpuInfo {
            model_name: cpu_info.brand_string(),
            cores: cpu_info.num_physical_cores(),
            logical_processors: num_cpus,
            max_frequency_mhz: Some(cpu_info.frequency() as f64),
        })
    }
    
    /// Detect memory information
    fn detect_memory() -> Result<MemoryInfo> {
        use sys_info;
        
        let mem_info = mem_info()?;
        
        Ok(MemoryInfo {
            total_gb: mem_info.total() as f64 / (1024.0 * 1024.0 * 1024.0),
            available_gb: mem_info.available() as f64 / (1024.0 * 1024.0 * 1024.0),
            swap_gb: mem_info.swap_total() as f64 / (1024.0 * 1024.0 * 1024.0),
        })
    }
    
    /// Detect GPU information (NVIDIA/AMD/Apple Silicon)
    fn detect_gpu() -> Result<Option<GpuInfo>> {
        // Try NVIDIA SMI first
        if let Ok(output) = Command::new("nvidia-smi")
            .arg("--query-gpu=name,memory.total,driver_version,cuda_vulkan_support")
            .arg("--format=csv,noheader")
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parts: Vec<&str> = stdout.trim().split(',').collect();
                
                if parts.len() >= 3 {
                    return Ok(Some(GpuInfo {
                        name: parts[0].trim().to_string(),
                        vram_gb: parts[1].trim().replace(" MiB", "").parse::<f64>().unwrap_or(0.0) / 1024.0,
                        driver_version: parts[2].trim().to_string(),
                        cuda_available: parts.get(3).map(|s| s.contains("Supported")).unwrap_or(false),
                        vulkan_available: false, // Would need additional detection
                    }));
                }
            }
        }
        
        // Try Apple Silicon (macOS)
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("system_profiler")
                .args(["SPDisplaysDataType"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("Apple M") || stdout.contains("Apple GPU") {
                    // Parse Apple GPU info (simplified)
                    return Ok(Some(GpuInfo {
                        name: "Apple Silicon GPU".to_string(),
                        vram_gb: 8.0, // Unified memory (simplified)
                        driver_version: "Integrated".to_string(),
                        cuda_available: false,
                        vulkan_available: true, // Metal support
                    }));
                }
            }
        }
        
        Ok(None)
    }
    
    /// Detect VS Code installation
    fn detect_vscode() -> Result<Option<VscodeInstallation>> {
        #[cfg(target_os = "windows")]
        let vscode_paths = vec![
            PathBuf::from(r"C:\Users\Program Files\Microsoft VS Code\Code.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Microsoft VS Code\Code.exe"),
        ];
        
        #[cfg(target_os = "macos")]
        let vscode_paths = vec![
            PathBuf::from("/Applications/Visual Studio Code.app"),
            PathBuf::from("/Applications/VSCodium.app"),
        ];
        
        #[cfg(target_os = "linux")]
        let vscode_paths = vec![
            PathBuf::from("/usr/bin/code"),
            PathBuf::from("/usr/local/bin/code"),
        ];
        
        for path in &vscode_paths {
            if path.exists() {
                // Get VS Code version
                let version = Command::new(path)
                    .arg("--version")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.to_string()))
                    .unwrap_or_else(|| "unknown".to_string());
                
                // Determine user data dir
                let user_data_dir = dirs::home_dir()
                    .map(|p| match std::env::var("VSCODE_APPDATA") {
                        Ok(val) => PathBuf::from(val),
                        Err(_) => {
                            #[cfg(windows)]
                            { p.join("AppData\\Roaming\\Code") }
                            #[cfg(macOS)]
                            { p.join("Library/Application Support/Code") }
                            #[cfg(linux)]
                            { p.join(".config/Code") }
                        }
                    })
                    .unwrap_or_default();
                
                let extensions_dir = user_data_dir.join("extensions");
                
                // Check if VS Code is running
                let is_running = Command::new(path)
                    .arg("--list-extensions") // This will fail if not running
                    .output()
                    .is_ok();
                
                return Ok(Some(VscodeInstallation {
                    path: path.clone(),
                    version,
                    channel: if path.to_string_lossy().contains("Insider") { 
                        "insider" 
                    } else { 
                        "stable" 
                    }.to_string(),
                    extensions_dir,
                    user_data_dir,
                    is_running,
                }));
            }
        }
        
        Ok(None)
    }
    
    /// Detect network capabilities
    async fn detect_network() -> Result<NetworkCapabilities> {
        // Check basic internet connectivity
        let has_internet = tokio::net::TcpStream::connect("8.8.8.8:53").await.is_ok();
        
        // Check specific endpoints (non-blocking, short timeout)
        let can_reach_openai = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect("api.openai.com:443")
        ).await.is_ok();
        
        let can_reach_deepseek = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect("api.deepseek.com:443")
        ).await.is_ok();
        
        // Check for proxy settings
        let proxy_detected = std::env::var("HTTP_PROXY").is_ok() ||
                           std::env::var("HTTPS_PROXY").is_ok() ||
                           std::env::var("http_proxy").is_ok() ||
                           std::env::var("https_proxy").is_ok();
        
        let proxy_url = std::env::var("HTTPS_PROXY")
            .or_else(|_| std::env::var("HTTP_PROXY"))
            .or_else(|_| std::env::var("https_proxy"))
            .or_else(|_| std::env::var("http_proxy"))
            .ok();
        
        Ok(NetworkCapabilities {
            has_internet,
            can_reach_openai,
            can_reach_deepseek,
            proxy_detected,
            proxy_url,
        })
    }
    
    /// Detect available LLM providers based on environment
    async fn detect_llm_providers(caps: &SystemCapabilities) -> Result<Vec<DetectedProvider>> {
        let mut providers = Vec::new();
        
        // 1. Deepseek (check API key)
        if let Ok(api_key) = std::env::var("DEEPSEEK_API_KEY") {
            providers.push(DetectedProvider {
                provider_type: "deepseek".to_string(),
                display_name: "DeepSeek".to_string(),
                is_configured: !api_key.is_empty(),
                api_key_source: Some("env:DEEPSEEK_API_KEY".to_string()),
                endpoint_url: Some("https://api.deepseek.com".to_string()),
                recommended_models: vec![
                    "deepseek-chat".to_string(),
                    "deepseek-coder".to_string(),
                ],
                priority: 0, // Highest priority if configured
            });
            
            info!("✅ DeepSeek provider detected (API key found)");
        } else {
            providers.push(DetectedProvider {
                provider_type: "deepseek".to_string(),
                display_name: "DeepSeek".to_string(),
                is_configured: false,
                api_key_source: None,
                endpoint_url: Some("https://api.deepseek.com".to_string()),
                recommended_models: vec!["deepseek-chat".to_string()],
                priority: 5,
            });
            
            debug!("⚠️  DeepSeek API key not found");
        }
        
        // 2. OpenAI (check API key)
        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            providers.push(DetectedProvider {
                provider_type: "openai-compatible".to_string(),
                display_name: "OpenAI".to_string(),
                is_configured: !api_key.is_empty(),
                api_key_source: Some("env:OPENAI_API_KEY".to_string()),
                endpoint_url: Some("https://api.openai.com/v1".to_string()),
                recommended_models: vec![
                    "gpt-4-turbo".to_string(),
                    "gpt-4".to_string(),
                    "gpt-3.5-turbo".to_string(),
                ],
                priority: 1,
            });
            
            info!("✅ OpenAI provider detected (API key found)");
        }
        
        // 3. Local vLLM (check if running on localhost)
        if caps.network.has_internet == false || 
           tokio::time::timeout(
               std::time::Duration::from_millis(500),
               tokio::net::TcpStream::connect("localhost:8000")
           ).await.is_ok() {
            providers.push(DetectedProvider {
                provider_type: "vllm".to_string(),
                display_name: "vLLM (Local)".to_string(),
                is_configured: true,
                api_key_source: Some("local deployment".to_string()),
                endpoint_url: Some("http://localhost:8000/v1".to_string()),
                recommended_models: vec![
                    "Qwen2.5-72B-Instruct".to_string(),
                    "Llama-3.1-70B".to_string(),
                ],
                priority: 2,
            });
            
            info!("✅ vLLM local server detected at localhost:8000");
        }
        
        // 4. llama.cpp (local inference)
        if std::path::Path::new("./models").exists() || 
           std::path::Path::new("~/.llama/models").expand_home().exists() {
            providers.push(DetectedProvider {
                provider_type: "llamacpp".to_string(),
                display_name: "llama.cpp (Local)".to_string(),
                is_configured: true,
                api_key_source: Some("local models".to_string()),
                endpoint_url: Some("http://localhost:8080".to_string()),
                recommended_models: vec![
                    "llama-3.1-8b".to_string(),
                    "mistral-7b".to_string(),
                ],
                priority: 3,
            });
            
            info!("✅ llama.cpp detected (local models found)");
        }
        
        // Sort by priority
        providers.sort_by_key(|p| p.priority);
        
        Ok(providers)
    }
    
    /// Generate optimal configuration based on detected capabilities
    pub fn generate_optimal_config(caps: &SystemCapabilities) -> crate::config::JcodeConfig {
        let mut config = crate::config::JcodeConfig::zero_config();
        
        // Select best available provider
        if let Some(best_provider) = caps.available_providers.iter()
            .find(|p| p.is_configured) 
        {
            config.llm.default_provider = best_provider.provider_type.clone();
            config.llm.default_model = best_provider.recommended_models.first()
                .cloned()
                .unwrap_or_else(|| "deepseek-chat".to_string());
            
            if let Some(endpoint) = &best_provider.endpoint_url {
                config.llm.providers.entry(best_provider.provider_type.clone())
                    .or_insert_with(crate::config::ProviderSpecificConfig::default)
                    .api_base_url = Some(endpoint.clone());
            }
        }
        
        // Optimize performance based on hardware
        if let Some(ref gpu) = caps.gpu {
            if gpu.cuda_available {
                // Can use GPU-accelerated features
                config.performance.worker_threads = (caps.cpu.cores / 2).max(2);
                config.rag.embedding_model = "text-embedding-ada-002".to_string(); // Use faster embedding
            } else if gpu.vram_gb >= 4.0 {
                // Has some GPU capability
                config.performance.stream_buffer_size = 4096; // Larger buffer for smoother streaming
            }
        }
        
        // Adjust for memory constraints
        if caps.memory.available_gb < 8.0 {
            config.performance.max_concurrent_requests = 10; // Reduce concurrency
            config.performance.enable_cache = false; // Disable cache to save memory
            config.logging.level = "warn".to_string(); // Less logging
        } else if caps.memory.available_gb < 16.0 {
            config.performance.max_concurrent_requests = 50;
        }
        
        // Configure VS Code integration if detected
        if let Some(ref vscode) = caps.vscode {
            config.vscode.vscode_executable_path = Some(vscode.path.to_string_lossy().to_string());
            config.vscode.auto_detect = true;
            
            if vscode.channel == "insider" {
                // Enable experimental features for Insider builds
                config.rest.enable_docs = true;
                config.logging.level = "debug".to_string();
            }
        }
        
        // Network-specific optimizations
        if caps.network.proxy_detected {
            // Configure timeouts longer when behind proxy
            config.llm.timeout_secs = 60;
            config.rest.timeout_secs = 120;
        }
        
        config
    }
}
