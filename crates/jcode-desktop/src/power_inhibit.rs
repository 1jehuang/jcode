use std::process::{Child, Command, Stdio};

const DISABLE_ENV: &str = "JCODE_DISABLE_POWER_INHIBIT";

/// Best-effort inhibitor that keeps Linux laptops awake while Jcode is actively
/// streaming/processing. On systemd systems this asks logind to ignore sleep and
/// lid-switch sleep requests for as long as the helper process is alive.
pub(crate) struct PowerInhibitor {
    child: Option<Child>,
    available: bool,
}

impl PowerInhibitor {
    pub(crate) fn new() -> Self {
        Self {
            child: None,
            available: cfg!(target_os = "linux") && std::env::var_os(DISABLE_ENV).is_none(),
        }
    }

    pub(crate) fn set_active(&mut self, active: bool) {
        if !self.available {
            return;
        }

        if active {
            self.acquire();
        } else {
            self.release();
        }
    }

    fn acquire(&mut self) {
        if self.child.as_mut().is_some_and(child_is_running) {
            return;
        }
        self.release();

        match build_inhibit_command().spawn() {
            Ok(child) => {
                self.child = Some(child);
            }
            Err(error) => {
                eprintln!("jcode: failed to acquire power inhibitor: {error}");
                self.available = false;
            }
        }
    }

    fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for PowerInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

fn child_is_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

fn build_inhibit_command() -> Command {
    let mut command = Command::new("systemd-inhibit");
    command
        .arg("--what=sleep:handle-lid-switch")
        .arg("--who=jcode")
        .arg("--why=Jcode is streaming or processing active work")
        .arg("sleep")
        .arg("infinity")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    #[test]
    fn linux_inhibitor_blocks_sleep_and_lid_switch() {
        let args = super::build_inhibit_command()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(args.contains(&"--what=sleep:handle-lid-switch".to_string()));
        assert!(args.contains(&"--who=jcode".to_string()));
        assert!(args.contains(&"sleep".to_string()));
        assert!(args.contains(&"infinity".to_string()));
    }
}
