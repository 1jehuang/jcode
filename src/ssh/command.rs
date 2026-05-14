use super::session::{SshConfig, SshSession};

pub struct SshCommand;

impl SshCommand {
    pub fn execute(args: &[String]) -> String {
        if args.is_empty() {
            return Self::usage().to_string();
        }

        match args[0].as_str() {
            "connect" => {
                if args.len() < 2 {
                    return "Usage: ssh connect <host> [--port PORT] [--user USER] [--identity FILE]".to_string();
                }
                let host = args[1].clone();
                let mut config = SshConfig::default();
                config.host = host;

                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--port" | "-p" => {
                            if i + 1 < args.len() {
                                config.port = args[i + 1].parse().unwrap_or(22);
                                i += 2;
                            } else { i += 1; }
                        }
                        "--user" | "-u" => {
                            if i + 1 < args.len() {
                                config.user = args[i + 1].clone();
                                i += 2;
                            } else { i += 1; }
                        }
                        "--identity" | "-i" => {
                            if i + 1 < args.len() {
                                config.identity_file = Some(std::path::PathBuf::from(&args[i + 1]));
                                i += 2;
                            } else { i += 1; }
                        }
                        _ => { i += 1; }
                    }
                }

                let mut session = SshSession::new(config);
                match session.connect() {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "exec" => {
                if args.len() < 3 {
                    return "Usage: ssh exec <host> <command>".to_string();
                }
                let host = &args[1];
                let cmd_args = &args[2..].join(" ");

                let config = SshConfig {
                    host: host.clone(),
                    ..Default::default()
                };

                let mut session = SshSession::new(config);
                match session.connect() {
                    Ok(_) => match session.execute(cmd_args) {
                        Ok(output) => format!(
                            "{}\n[Exit code: {:?}, Duration: {:?}]",
                            output.stdout, output.exit_code, output.duration
                        ),
                        Err(e) => format!("Execution error: {}", e),
                    },
                    Err(e) => format!("Connection error: {}", e),
                }
            }
            "upload" => {
                if args.len() < 4 {
                    return "Usage: ssh upload <host> <local_path> <remote_path>".to_string();
                }
                let config = SshConfig {
                    host: args[1].clone(),
                    ..Default::default()
                };
                let session = SshSession::new(config);
                match session.upload(
                    &std::path::PathBuf::from(&args[2]),
                    &std::path::PathBuf::from(&args[3]),
                ) {
                    Ok(_) => "File uploaded successfully".to_string(),
                    Err(e) => format!("Upload error: {}", e),
                }
            }
            "download" => {
                if args.len() < 4 {
                    return "Usage: ssh download <host> <remote_path> <local_path>".to_string();
                }
                let config = SshConfig {
                    host: args[1].clone(),
                    ..Default::default()
                };
                let session = SshSession::new(config);
                match session.download(
                    &std::path::PathBuf::from(&args[2]),
                    &std::path::PathBuf::from(&args[3]),
                ) {
                    Ok(_) => "File downloaded successfully".to_string(),
                    Err(e) => format!("Download error: {}", e),
                }
            }
            _ => format!("Unknown subcommand: {}. {}", args[0], Self::usage()),
        }
    }

    fn usage() -> &'static str {
        r#"SSH Remote Commands:
  ssh connect <host> [options]   - Connect to remote host
  ssh exec <host> <command>      - Execute command on remote
  ssh upload <host> <local> <remote> - Upload file to remote
  ssh download <host> <remote> <local> - Download file from remote

Options:
  --port, -p <port>     SSH port (default: 22)
  --user, -u <user>     SSH user (default: current user)
  --identity, -i <file> SSH private key file
"#
    }
}
