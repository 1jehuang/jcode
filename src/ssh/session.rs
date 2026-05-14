use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<PathBuf>,
    pub connect_timeout: Duration,
}

impl Default for SshConfig {
    fn default() -> Self {
        SshConfig {
            host: "localhost".to_string(),
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
            identity_file: None,
            connect_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
pub struct SshSession {
    config: SshConfig,
    connected: bool,
    connected_at: Option<Instant>,
}

impl SshSession {
    pub fn new(config: SshConfig) -> Self {
        SshSession {
            config,
            connected: false,
            connected_at: None,
        }
    }

    pub fn connect(&mut self) -> Result<String, String> {
        if self.connected {
            return Err("Already connected".to_string());
        }

        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
           .arg(format!("ConnectTimeout={}", self.config.connect_timeout.as_secs()))
           .arg("-o")
           .arg("BatchMode=yes")
           .arg("-p")
           .arg(self.config.port.to_string());

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(format!("{}@{}", self.config.user, self.config.host))
           .arg("echo 'connected'")
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let output = cmd.output().map_err(|e| format!("Failed to execute ssh: {}", e))?;

        if output.status.success() {
            self.connected = true;
            self.connected_at = Some(Instant::now());
            Ok("Connected successfully".to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Connection failed: {}", stderr))
        }
    }

    pub fn execute(&self, command: &str) -> Result<SshOutput, String> {
        if !self.connected {
            return Err("Not connected. Call connect() first.".to_string());
        }

        let remote_cmd = format!("{}@{}", self.config.user, self.config.host);

        let mut cmd = Command::new("ssh");
        cmd.arg("-p")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(remote_cmd)
           .arg(command)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let start = Instant::now();
        let output = cmd.output().map_err(|e| format!("Execution failed: {}", e))?;
        let duration = start.elapsed();

        Ok(SshOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            duration,
        })
    }

    pub fn execute_streaming<F>(&self, command: &str, mut on_line: F) -> Result<(), String>
    where
        F: FnMut(&str),
    {
        if !self.connected {
            return Err("Not connected. Call connect() first.".to_string());
        }

        let remote_cmd = format!("{}@{}", self.config.user, self.config.host);

        let mut cmd = Command::new("ssh");
        cmd.arg("-p")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(remote_cmd)
           .arg(command)
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|e| format!("Failed to spawn process: {}", e))?;
        let stdout = child.stdout.ok_or("Failed to capture stdout")?;

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => on_line(&line),
                Err(e) => return Err(format!("Read error: {}", e)),
            }
        }

        Ok(())
    }

    pub fn upload(&self, local_path: &PathBuf, remote_path: &PathBuf) -> Result<(), String> {
        let remote_dest = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(local_path)
           .arg(remote_dest)
           .status()
           .map_err(|e| format!("Upload failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP upload failed".to_string()) }
           })
    }

    pub fn download(&self, remote_path: &PathBuf, local_path: &PathBuf) -> Result<(), String> {
        let remote_src = format!("{}@{}:{}", self.config.user, self.config.host, remote_path.display());

        let mut cmd = Command::new("scp");
        cmd.arg("-P")
           .arg(self.config.port.to_string())
           .arg("-o")
           .arg("BatchMode=yes");

        if let Some(ref identity) = self.config.identity_file {
            cmd.arg("-i").arg(identity);
        }

        cmd.arg(remote_src)
           .arg(local_path)
           .status()
           .map_err(|e| format!("Download failed: {}", e))
           .and_then(|status| {
               if status.success() { Ok(()) } else { Err("SCP download failed".to_string()) }
           })
    }

    pub fn disconnect(&mut self) -> Result<String, String> {
        if !self.connected {
            return Err("Not connected".to_string());
        }
        self.connected = false;
        self.connected_at = None;
        Ok("Disconnected successfully".to_string())
    }

    pub fn is_connected(&self) -> bool { self.connected }

    pub fn uptime(&self) -> Option<Duration> {
        self.connected_at.map(|t| t.elapsed())
    }
}

#[derive(Debug)]
pub struct SshOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration: Duration,
}
