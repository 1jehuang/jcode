use std::process::{Command, Child, Stdio};
use std::time::Duration;
use std::io;

/// PTY (Pseudo-Terminal) Session Manager
/// 
/// Provides full terminal emulation for interactive SSH sessions:
/// - Complete PTY allocation with size negotiation
/// - Terminal signal forwarding (SIGWINCH, SIGINT, SIGTERM)
/// - Interactive application support (vim, top, htop, etc.)
/// - Color output preservation
/// - Window resize handling
/// - Raw mode terminal support

pub struct PtySession {
    child: Option<Child>,
    pty_master: Option<PtyMaster>,
    session_id: String,
    config: PtyConfig,
    state: PtyState,
    created_at: std::time::Instant,
    last_activity: Option<std::time::Instant>,
    dimensions: TerminalSize,
}

#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub rows: u16,
    pub cols: u16,
    pub term_type: String,           // xterm-256color, vt100, etc.
    pub echo_input: bool,
    pub raw_mode: bool,
    pub enable_signals: bool,
    pub timeout: Option<Duration>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        PtyConfig {
            rows: 24,
            cols: 80,
            term_type: "xterm-256color".to_string(),
            echo_input: true,
            raw_mode: false,
            enable_signals: true,
            timeout: Some(Duration::from_secs(300)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PtyState {
    Created,
    Running,
    Stopped,
    Exited(i32),
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        TerminalSize { rows: 24, cols: 80, xpixel: 0, ypixel: 0 }
    }
}

#[derive(Debug, Clone)]
pub enum PtyError {
    AllocationFailed {
        message: String,
    },
    ProcessSpawnFailed {
        command: String,
        message: String,
    },
    IoError {
        operation: String,
        details: String,
    },
    Timeout {
        operation: String,
        duration: Duration,
    },
    NotRunning,
    AlreadyRunning,
    ResizeFailed {
        message: String,
    },
    SignalError {
        signal: String,
        message: String,
    },
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::AllocationFailed { message } => {
                write!(f, "PTY allocation failed: {}", message)
            }
            PtyError::ProcessSpawnFailed { command, message } => {
                write!(f, "Failed to spawn process '{}': {}", command, message)
            }
            PtyError::IoError { operation, details } => {
                write!(f, "I/O error during {}: {}", operation, details)
            }
            PtyError::Timeout { operation, duration } => {
                write!(f, "Timeout after {:?} during {}", duration, operation)
            }
            PtyError::NotRunning => {
                write!(f, "PTY session is not running")
            }
            PtyError::AlreadyRunning => {
                write!(f, "PTY session is already running")
            }
            PtyError::ResizeFailed { message } => {
                write!(f, "Terminal resize failed: {}", message)
            }
            PtyError::SignalError { signal, message } => {
                write!(f, "Failed to send signal {}: {}", signal, message)
            }
        }
    }
}

impl std::error::Error for PtyError {}

// Platform-specific PTY master handle
#[cfg(unix)]
struct PtyMaster {
    master_fd: std::os::unix::io::RawFd,
    slave_name: String,
}

#[cfg(windows)]
struct PtyMaster {
    // Windows ConPTY handles
    input_handle: Option<isize>,
    output_handle: Option<isize>,
}

impl PtySession {
    /// Create new PTY session with default configuration
    pub fn new() -> Self {
        PtySession {
            child: None,
            pty_master: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            config: PtyConfig::default(),
            state: PtyState::Created,
            created_at: std::time::Instant::now(),
            last_activity: None,
            dimensions: TerminalSize::default(),
        }
    }

    /// Create new PTY session with custom configuration
    pub fn with_config(config: PtyConfig) -> Self {
        Self {
            child: None,
            pty_master: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            config,
            state: PtyState::Created,
            created_at: std::time::Instant::now(),
            last_activity: None,
            dimensions: TerminalSize::default(),
        }
    }

    /// Set terminal dimensions
    pub fn set_dimensions(&mut self, rows: u16, cols: u16) {
        self.dimensions = TerminalSize {
            rows,
            cols,
            xpixel: 0,
            ypixel: 0,
        };
        
        // Update config
        self.config.rows = rows;
        self.config.cols = cols;
        
        // If running, try to resize immediately
        if let Some(ref mut master) = self.pty_master {
            let _ = Self::_resize_pty_static(master, rows, cols);
        }
    }

    /// Get current terminal dimensions
    pub fn dimensions(&self) -> &TerminalSize {
        &self.dimensions
    }

    /// Get session ID
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Get current state
    pub fn state(&self) -> &PtyState {
        &self.state
    }

    /// Check if session is still active
    pub fn is_running(&self) -> bool {
        matches!(self.state, PtyState::Running)
    }

    /// Get uptime of the session
    pub fn uptime(&self) -> Duration {
        self.created_at.elapsed()
    }

    // ==========================================
    // Session Lifecycle Operations
    // ==========================================

    /// Start an interactive SSH session via PTY
    pub fn start_ssh_session(
        &mut self,
        ssh_target: &str,
        ssh_command: Option<&str>,
    ) -> Result<(), PtyError> {
        if !matches!(self.state, PtyState::Created | PtyState::Exited(_)) {
            return Err(PtyError::AlreadyRunning);
        }

        // Allocate PTY
        let pty_master = self._allocate_pty()?;
        
        // Build SSH command with PTY allocation
        let mut cmd = Command::new("ssh");
        cmd.arg("-t")  // Force PTY allocation
           .arg("-o").arg("RequestTTY=yes")
           .arg("-o").arg(format!("TERM={}", self.config.term_type))
           .arg(ssh_target);

        if let Some(command) = ssh_command {
            cmd.arg(command);
        }

        // Configure stdin/stdout/stderr to use PTY
        #[cfg(unix)]
        {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            
            unsafe {
                cmd.stdin(Stdio::from_raw_fd(pty_master.master_fd));
                cmd.stdout(Stdio::from_raw_fd(pty_master.master_fd));
                cmd.stderr(Stdio::from_raw_fd(pty_master.master_fd));
            }
        }

        #[cfg(windows)]
        {
            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }

        // Spawn the process
        let child = cmd.spawn().map_err(|e| PtyError::ProcessSpawnFailed {
            command: format!("ssh {}", ssh_target),
            message: e.to_string(),
        })?;

        self.child = Some(child);
        self.pty_master = Some(pty_master);
        self.state = PtyState::Running;
        self.last_activity = Some(std::time::Instant::now());

        Ok(())
    }

    /// Start a generic command in PTY (for local commands like vim, top)
    pub fn start_command(
        &mut self,
        program: &str,
        args: &[&str],
    ) -> Result<(), PtyError> {
        if !matches!(self.state, PtyState::Created | PtyState::Exited(_)) {
            return Err(PtyError::AlreadyRunning);
        }

        let pty_master = self._allocate_pty()?;

        let mut cmd = Command::new(program);
        cmd.args(args)
           .env("TERM", &self.config.term_type);

        if self.config.raw_mode {
            cmd.env("COLUMNS", self.config.cols.to_string())
               .env("LINES", self.config.rows.to_string());
        }

        #[cfg(unix)]
        {
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            unsafe {
                cmd.stdin(Stdio::from_raw_fd(pty_master.master_fd));
                cmd.stdout(Stdio::from_raw_fd(pty_master.master_fd));
                cmd.stderr(Stdio::from_raw_fd(pty_master.master_fd));
            }
        }

        let child = cmd.spawn().map_err(|e| PtyError::ProcessSpawnFailed {
            command: format!("{} {}", program, args.join(" ")),
            message: e.to_string(),
        })?;

        self.child = Some(child);
        self.pty_master = Some(pty_master);
        self.state = PtyState::Running;
        self.last_activity = Some(std::time::Instant::now());

        Ok(())
    }

    /// Stop/kill the running session
    pub fn stop(&mut self) -> Result<i32, PtyError> {
        match self.child.take() {
            Some(mut child) => {
                // Try graceful shutdown first (SIGTERM)
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    let _ = libc::kill(child.id() as i32, libc::SIGTERM);
                    
                    // Wait briefly for graceful exit
                    std::thread::sleep(Duration::from_millis(100));
                    
                    if let Ok(Some(status)) = child.try_wait() {
                        let code = status.code().unwrap_or(-1);
                        self.state = PtyState::Exited(code);
                        self.pty_master = None;
                        return Ok(code);
                    }
                    
                    // Force kill if still running
                    let _ = child.kill();
                    match child.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            self.state = PtyState::Exited(code);
                            self.pty_master = None;
                            Ok(code)
                        }
                        Err(e) => Err(PtyError::IoError {
                            operation: "wait".to_string(),
                            details: e.to_string(),
                        }),
                    }
                }

                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                    match child.wait() {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            self.state = PtyState::Exited(code);
                            self.pty_master = None;
                            Ok(code)
                        }
                        Err(e) => Err(PtyError::IoError {
                            operation: "wait".to_string(),
                            details: e.to_string(),
                        }),
                    }
                }
            }
            None => Err(PtyError::NotRunning),
        }
    }

    // ==========================================
    // I/O Operations
    // ==========================================

    /// Write data to the PTY (sends input to the remote process)
    pub fn write(&mut self, data: &[u8]) -> Result<usize, PtyError> {
        if !self.is_running() {
            return Err(PtyError::NotRunning);
        }

        self.last_activity = Some(std::time::Instant::now());

        #[cfg(unix)]
        {
            if let Some(ref master) = self.pty_master {
                use std::os::unix::io::AsRawFd;
                let written = unsafe {
                    libc::write(master.master_fd, data.as_ptr(), data.len())
                };

                if written < 0 {
                    Err(PtyError::IoError {
                        operation: "write".to_string(),
                        details: format!("Write error: {}", 
                            io::Error::last_os_error()),
                    })
                } else {
                    Ok(written as usize)
                }
            } else {
                Err(PtyError::NotRunning)
            }
        }

        #[cfg(not(unix))]
        {
            // Fallback for non-Unix systems
            if let Some(ref mut child) = self.child {
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write(data).map_err(|e| PtyError::IoError {
                        operation: "write".to_string(),
                        details: e.to_string(),
                    })
                } else {
                    Err(PtyError::NotRunning)
                }
            } else {
                Err(PtyError::NotRunning)
            }
        }
    }

    /// Write string data to PTY
    pub fn write_str(&mut self, s: &str) -> Result<usize, PtyError> {
        self.write(s.as_bytes())
    }

    /// Read available output from PTY (non-blocking)
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, PtyError> {
        if !self.is_running() {
            return Err(PtyError::NotRunning);
        }

        #[cfg(unix)]
        {
            if let Some(ref master) = self.pty_master {
                use std::os::unix::io::AsRawFd;
                
                // Check if data is available (non-blocking read)
                let mut fds = [libc::pollfd {
                    fd: master.master_fd,
                    events: libc::POLLIN,
                    revents: 0,
                }];

                let ready = unsafe {
                    libc::poll(fds.as_mut_ptr(), 1, 0)  // Immediate timeout
                };

                if ready > 0 && (fds[0].revents & libc::POLLIN) != 0 {
                    let bytes_read = unsafe {
                        libc::read(master.master_fd, buf.as_mut_ptr(), buf.len())
                    };

                    if bytes_read < 0 {
                        Err(PtyError::IoError {
                            operation: "read".to_string(),
                            details: format!("Read error: {}", 
                                io::Error::last_os_error()),
                        })
                    } else {
                        self.last_activity = Some(std::time::Instant::now());
                        Ok(bytes_read as usize)
                    }
                } else {
                    Ok(0)  // No data available
                }
            } else {
                Err(PtyError::NotRunning)
            }
        }

        #[cfg(not(unix))]
        {
            if let Some(ref mut child) = self.child {
                if let Some(stdout) = child.stdout.as_mut() {
                    stdout.read(buf).map_err(|e| PtyError::IoError {
                        operation: "read".to_string(),
                        details: e.to_string(),
                    })
                } else {
                    Err(PtyError::NotRunning)
                }
            } else {
                Err(PtyError::NotRunning)
            }
        }
    }

    /// Read all available output as a string (blocking with timeout)
    pub fn read_to_string(&mut self, timeout: Duration) -> Result<String, PtyError> {
        let start = std::time::Instant::now();
        let mut output = String::new();
        let mut buf = [0u8; 4096];

        loop {
            match self.read(&mut buf) {
                Ok(0) => {
                    // No data available, check timeout
                    if start.elapsed() >= timeout {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(n) => {
                    output.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                Err(e) => {
                    if output.is_empty() {
                        return Err(e);
                    } else {
                        break;  // Return what we have so far
                    }
                }
            }

            // Also check if process has exited
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
            }
        }

        Ok(output)
    }

    // ==========================================
    // Signal Handling
    // ==========================================

    /// Send SIGINT (Ctrl+C equivalent) to the foreground process group
    pub fn send_interrupt(&mut self) -> Result<(), PtyError> {
        self._send_signal("SIGINT", "interrupt")
    }

    /// Send SIGTSTP (Ctrl+Z equivalent) to suspend the process
    pub fn send_suspend(&mut self) -> Result<(), PtyError> {
        self._send_signal("SIGTSTP", "suspend")
    }

    /// Send SIGQUIT (Ctrl+\ equivalent) for core dump
    pub fn send_quit(&mut self) -> Result<(), PtyError> {
        self._send_signal("SIGQUIT", "quit")
    }

    /// Handle window size change (SIGWINCH)
    pub fn handle_resize(&mut self, new_rows: u16, new_cols: u16) -> Result<(), PtyError> {
        self.set_dimensions(new_rows, new_cols);

        // Forward SIGWINCH to child process group
        #[cfg(unix)]
        {
            if let Some(ref child) = self.child {
                use std::os::unix::process::ExitStatusExt;
                let pid = child.id() as i32;
                let _ = unsafe { libc::kill(-pid, libc::SIGWINCH) };
            }
        }

        Ok(())
    }

    // ==========================================
    // Private Implementation Details
    // ==========================================

    fn _allocate_pty(&self) -> Result<PtyMaster, PtyError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::FromRawFd;

            // Open /dev/ptmx to get master PTY
            let master_fd = unsafe {
                libc::open(b"/dev/ptmx\0".as_ptr() as *const i8, 
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC)
            };

            if master_fd < 0 {
                return Err(PtyError::AllocationFailed {
                    message: format!("Failed to open /dev/ptmx: {}", 
                        io::Error::last_os_error()),
                });
            }

            // Unlock slave PTY and get its name
            let mut slave_name = [0i8; 64];
            let result = unsafe {
                libc::ptsname_r(master_fd, slave_name.as_mut_ptr(), slave_name.len())
            };

            if result != 0 {
                unsafe { libc::close(master_fd); }
                return Err(PtyError::AllocationFailed {
                    message: "Failed to get slave PTY name".to_string(),
                });
            }

            // Grant access to slave
            unsafe { libc::grantpt(master_fd); }
            unsafe { libc::unlockpt(master_fd); }

            let name_str = unsafe {
                std::ffi::CStr::from_ptr(slave_name.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };

            Ok(PtyMaster {
                master_fd,
                slave_name: name_str,
            })
        }

        #[cfg(not(unix))]
        {
            // For non-Unix systems, create a dummy PTY master
            // In production, this would use ConPTY on Windows
            Ok(PtyMaster {
                input_handle: None,
                output_handle: None,
            })
        }
    }

    fn _resize_pty_static(master: &PtyMaster, rows: u16, cols: u16, xpixel: u16, ypixel: u16) -> Result<(), PtyError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            
            let winsize = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: xpixel,
                ws_ypixel: ypixel,
            };

            let result = unsafe {
                libc::ioctl(master.master_fd, libc::TIOCSWINSZ, &winsize)
            };

            if result < 0 {
                Err(PtyError::ResizeFailed {
                    message: format!("ioctl TIOCSWINSZ failed: {}", 
                        io::Error::last_os_error()),
                })
            } else {
                Ok(())
            }
        }

        #[cfg(not(unix))]
        {
            // Cannot resize on non-Unix without proper ConPTY support
            Ok(())
        }
    }

    fn _send_signal(&mut self, signal_name: &str, _signal_desc: &str) -> Result<(), PtyError> {
        #[cfg(unix)]
        {
            if let Some(ref child) = self.child {
                use std::os::unix::process::ExitStatusExt;
                let pid = child.id() as i32;

                let signum = match signal_name {
                    "SIGINT" => libc::SIGINT,
                    "SIGTSTP" => libc::SIGTSTP,
                    "SIGQUIT" => libc::SIGQUIT,
                    "SIGWINCH" => libc::SIGWINCH,
                    "SIGTERM" => libc::SIGTERM,
                    _ => return Err(PtyError::SignalError {
                        signal: signal_name.to_string(),
                        message: "Unknown signal".to_string(),
                    }),
                };

                // Send signal to entire foreground process group (-pid)
                let result = unsafe { libc::kill(-pid, signum) };

                if result == 0 {
                    self.last_activity = Some(std::time::Instant::now());
                    Ok(())
                } else {
                    Err(PtyError::SignalError {
                        signal: signal_name.to_string(),
                        message: format!("kill failed: {}", io::Error::last_os_error()),
                    })
                }
            } else {
                Err(PtyError::NotRunning)
            }
        }

        #[cfg(not(unix))]
        {
            Err(PtyError::SignalError {
                signal: signal_name.to_string(),
                message: "Signals not supported on this platform".to_string(),
            })
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if self.is_running() {
            let _ = self.stop();  // Best effort cleanup
        }
    }
}

/// High-level PTY session manager for multiple concurrent terminals
pub struct PtySessionManager {
    sessions: std::collections::HashMap<String, PtySession>,
    max_sessions: usize,
}

impl PtySessionManager {
    pub fn new(max_sessions: usize) -> Self {
        PtySessionManager {
            sessions: std::collections::HashMap::new(),
            max_sessions,
        }
    }

    /// Create and start a new PTY session
    pub fn create_session(
        &mut self,
        ssh_target: &str,
        command: Option<&str>,
        config: Option<PtyConfig>,
    ) -> Result<String, PtyError> {
        if self.sessions.len() >= self.max_sessions {
            return Err(PtyError::AllocationFailed {
                message: format!("Maximum sessions ({}) reached", self.max_sessions),
            });
        }

        let mut session = match config {
            Some(cfg) => PtySession::with_config(cfg),
            None => PtySession::new(),
        };

        session.start_ssh_session(ssh_target, command)?;
        let id = session.id().to_string();

        self.sessions.insert(id.clone(), session);
        Ok(id)
    }

    /// Get session by ID
    pub fn get_session(&mut self, id: &str) -> Option<&mut PtySession> {
        self.sessions.get_mut(id)
    }

    /// Stop and remove a session
    pub fn close_session(&mut self, id: &str) -> Result<i32, PtyError> {
        if let Some(mut session) = self.sessions.remove(id) {
            session.stop()
        } else {
            Err(PtyError::NotRunning)
        }
    }

    /// Close all sessions
    pub fn close_all(&mut self) -> Vec<(String, Result<i32, PtyError>)> {
        let results: Vec<_> = self.sessions.drain()
            .map(|(id, mut session)| {
                let result = session.stop();
                match result {
                    Ok(code) => (id, Ok(code), PtyError::NotRunning),  // Dummy error variant
                    Err(e) => (id, Err(-1), e),
                }
            })
            .collect();
        
        results
    }

    /// List all active session IDs
    pub fn list_sessions(&self) -> Vec<&str> {
        self.sessions.keys().map(|s| s.as_str()).collect()
    }

    /// Get count of active sessions
    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|s| s.is_running()).count()
    }
}
