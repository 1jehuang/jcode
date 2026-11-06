use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    time::Duration,
};
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, WaitForSingleObject,
};

#[repr(C)]
#[derive(Default)]
struct JobAccounting {
    user: i64,
    kernel: i64,
    period_user: i64,
    period_kernel: i64,
    faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryInformationJobObject(
        job: *mut std::ffi::c_void,
        class: i32,
        info: *mut std::ffi::c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
}

pub(super) struct NativeProcess {
    process: OwnedHandle,
    job: Option<OwnedHandle>,
    broker: Option<super::windows_native_launch::Broker>,
    terminate_on_request: bool,
}

pub(super) struct ShutdownGuard {
    job: OwnedHandle,
    terminate_on_drop: bool,
    armed: bool,
}

impl ShutdownGuard {
    pub(super) fn job(&self) -> &OwnedHandle {
        &self.job
    }
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if self.armed && self.terminate_on_drop {
            if let Err(error) = NativeProcess::terminate_job(&self.job) {
                crate::logging::warn(&format!(
                    "MCP cancelled shutdown Job termination failed: {error}"
                ));
            }
        }
    }
}

impl NativeProcess {
    pub(super) fn shutdown_guard(&self) -> io::Result<ShutdownGuard> {
        Ok(ShutdownGuard {
            job: self
                .job
                .as_ref()
                .ok_or_else(|| io::Error::other("missing owned MCP Job"))?
                .try_clone()?,
            terminate_on_drop: self.terminate_on_request,
            armed: true,
        })
    }
    pub(super) fn active_job_processes(job: &OwnedHandle) -> io::Result<u32> {
        let mut info = JobAccounting::default();
        if unsafe {
            QueryInformationJobObject(
                job.as_raw_handle(),
                1,
                (&mut info as *mut JobAccounting).cast(),
                std::mem::size_of::<JobAccounting>() as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.active)
    }

    pub(super) async fn wait_job_empty(job: &OwnedHandle, bound: Duration) -> io::Result<()> {
        tokio::time::timeout(bound, async {
            loop {
                if Self::active_job_processes(job)? == 0 {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP job still contains active processes",
            )
        })?
    }
    pub(super) fn new(
        process: OwnedHandle,
        job: OwnedHandle,
        broker: super::windows_native_launch::Broker,
        terminate_on_request: bool,
    ) -> Self {
        Self {
            process,
            job: Some(job),
            broker: Some(broker),
            terminate_on_request,
        }
    }

    #[cfg(feature = "test-support")]
    pub(super) fn resume_test_broker(&mut self) -> io::Result<()> {
        self.broker
            .as_mut()
            .ok_or_else(|| io::Error::other("missing broker"))?
            .resume_for_test()
    }

    pub(super) fn id(&self) -> io::Result<u32> {
        let id = unsafe { GetProcessId(self.process.as_raw_handle()) };
        if id == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(id)
        }
    }

    pub(super) fn broker_id(&self) -> io::Result<u32> {
        self.broker
            .as_ref()
            .ok_or_else(|| io::Error::other("missing owned broker"))?
            .id()
    }

    pub(super) fn try_wait(&self) -> io::Result<Option<u32>> {
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(Some(code))
                }
            }
            258 => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub(super) fn take_job(&mut self) -> Option<OwnedHandle> {
        self.job.take()
    }

    #[cfg(feature = "test-support")]
    pub(super) fn test_job_handle(&self) -> Option<usize> {
        self.job.as_ref().map(|job| job.as_raw_handle() as usize)
    }

    #[cfg(feature = "test-support")]
    pub(super) fn test_owned_handles(&self) -> io::Result<(usize, usize, usize, u32, u32)> {
        Ok((
            self.test_job_handle()
                .ok_or_else(|| io::Error::other("missing Job"))?,
            self.process.as_raw_handle() as usize,
            self.broker
                .as_ref()
                .ok_or_else(|| io::Error::other("missing broker"))?
                .test_raw_handle(),
            self.id()?,
            self.broker_id()?,
        ))
    }

    pub(super) fn terminate_owned_job(&self, job: &OwnedHandle) -> io::Result<()> {
        if self.terminate_on_request {
            Self::terminate_job(job)
        } else {
            Ok(())
        }
    }

    pub(super) fn terminate_job(job: &OwnedHandle) -> io::Result<()> {
        if unsafe { TerminateJobObject(job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn start_kill(&self) -> io::Result<()> {
        if !self.terminate_on_request {
            return Ok(());
        }
        match self.job.as_ref() {
            Some(job) => Self::terminate_job(job),
            None => Ok(()),
        }
    }
    pub(super) async fn wait(&self, bound: Duration) -> io::Result<u32> {
        tokio::time::timeout(bound, async {
            loop {
                if let Some(code) = self.try_wait()? {
                    return Ok(code);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MCP process wait timed out"))?
    }

    pub(super) async fn kill(&self, bound: Duration) -> io::Result<u32> {
        self.start_kill()?;
        self.wait(bound).await
    }
}
