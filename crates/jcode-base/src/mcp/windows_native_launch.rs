use super::windows_native_process::NativeProcess;
use std::{
    ffi::{OsStr, c_void},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};

type Handle = *mut c_void;
#[cfg(feature = "test-support")]
static SPAWN_OBSERVER: std::sync::Mutex<Option<fn()>> = std::sync::Mutex::new(None);
#[cfg(feature = "test-support")]
pub(super) fn test_set_spawn_observer(observer: Option<fn()>) {
    *SPAWN_OBSERVER
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = observer;
}
include!("windows_native_launch_ffi.rs");

pub(super) struct Broker {
    process: OwnedHandle,
    thread: OwnedHandle,
    resume_on_drop: bool,
}
impl Broker {
    #[cfg(feature = "test-support")]
    pub(super) fn test_raw_handle(&self) -> usize {
        self.process.as_raw_handle() as usize
    }
    pub(super) fn id(&self) -> io::Result<u32> {
        let id = unsafe {
            windows_sys::Win32::System::Threading::GetProcessId(self.process.as_raw_handle())
        };
        if id == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(id)
        }
    }
    #[cfg(feature = "test-support")]
    pub(super) fn resume_for_test(&mut self) -> io::Result<()> {
        if !self.resume_on_drop {
            return Err(io::Error::other("production broker must remain suspended"));
        }
        if unsafe { ResumeThread(self.thread.as_raw_handle()) } != 1 {
            return Err(io::Error::last_os_error());
        }
        self.resume_on_drop = false;
        Ok(())
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        if self.resume_on_drop {
            unsafe {
                ResumeThread(self.thread.as_raw_handle());
            }
        }
    }
}

struct RemoteHandle<'a> {
    broker: &'a OwnedHandle,
    raw: Handle,
}
impl<'a> RemoteHandle<'a> {
    fn new(source: &OwnedHandle, broker: &'a OwnedHandle) -> io::Result<Self> {
        let mut raw = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source.as_raw_handle(),
                broker.as_raw_handle(),
                &mut raw,
                0,
                1,
                2,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { broker, raw })
    }
    fn close(&mut self) -> io::Result<()> {
        if self.raw.is_null() {
            return Ok(());
        }
        let raw = std::mem::replace(&mut self.raw, std::ptr::null_mut());
        let mut local = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                self.broker.as_raw_handle(),
                raw,
                GetCurrentProcess(),
                &mut local,
                0,
                0,
                3,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        drop(handle(local)?);
        Ok(())
    }
}
impl Drop for RemoteHandle<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            crate::logging::warn(&format!("MCP remote handle cleanup failed: {error}"));
        }
    }
}

pub(super) struct Spawned {
    pub process: NativeProcess,
    pub stdin: tokio::process::ChildStdin,
    pub stdout: tokio::process::ChildStdout,
    pub stderr: tokio::process::ChildStderr,
}

struct Attributes {
    storage: Vec<usize>,
    ready: bool,
}
impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut size);
        }
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = Self {
            storage: vec![0; size.div_ceil(std::mem::size_of::<usize>())],
            ready: false,
        };
        if unsafe { InitializeProcThreadAttributeList(value.ptr(), count, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        value.ready = true;
        Ok(value)
    }
    fn ptr(&mut self) -> Handle {
        self.storage.as_mut_ptr().cast()
    }
    fn set(&mut self, key: usize, handles: &mut [Handle]) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.ptr(),
                0,
                key,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.ready {
            unsafe {
                DeleteProcThreadAttributeList(self.ptr());
            }
        }
    }
}

fn handle(raw: Handle) -> io::Result<OwnedHandle> {
    if raw.is_null() || raw as isize == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<u16> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in process input",
        ));
    }
    result.push(0);
    Ok(result)
}
fn cwd_wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let original = wide(value)?;
    let candidate = if original.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
        let mut value = vec![92, 92];
        value.extend_from_slice(&original[8..]);
        Some(value)
    } else if original.starts_with(&[92, 92, 63, 92]) {
        Some(original[4..].to_vec())
    } else {
        None
    };
    if let Some(candidate) = candidate {
        let mut normalized = vec![0u16; candidate.len()];
        let n = unsafe {
            GetFullPathNameW(
                candidate.as_ptr(),
                normalized.len() as u32,
                normalized.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        } as usize;
        if n + 1 == candidate.len() && normalized == candidate {
            return Ok(candidate);
        }
    }
    Ok(original)
}
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut security = Security {
        length: std::mem::size_of::<Security>() as u32,
        descriptor: std::ptr::null_mut(),
        inherit: 0,
    };
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, &mut security, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((handle(read)?, handle(write)?))
}
fn non_inheritable(handle: &OwnedHandle) -> io::Result<()> {
    if unsafe { SetHandleInformation(handle.as_raw_handle(), 1, 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// Inputs must already match the desired Windows command resolution and quoting semantics.
pub(super) fn spawn(
    app: &OsStr,
    line: &OsStr,
    cwd: Option<&OsStr>,
    env: Option<&mut [u16]>,
) -> io::Result<Spawned> {
    spawn_inner(app, line, cwd, env, app, true)
}

#[cfg(feature = "test-support")]
pub(super) fn spawn_natural_test(
    app: &OsStr,
    line: &OsStr,
    cwd: Option<&OsStr>,
    env: Option<&mut [u16]>,
    broker_line: &OsStr,
) -> io::Result<Spawned> {
    spawn_inner(app, line, cwd, env, broker_line, false)
}

fn spawn_inner(
    app: &OsStr,
    line: &OsStr,
    cwd: Option<&OsStr>,
    env: Option<&mut [u16]>,
    broker_line: &OsStr,
    kill_on_close: bool,
) -> io::Result<Spawned> {
    if !Path::new(app).is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "executable must be absolute",
        ));
    }
    let app = wide(app)?;
    let mut line = wide(line)?;
    let cwd = cwd.map(cwd_wide).transpose()?;
    if let Some(block) = env.as_ref() {
        if !block.ends_with(&[0, 0]) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "environment block must end with two NULs",
            ));
        }
    }
    let env_ptr = env.map_or(std::ptr::null_mut(), |block| block.as_mut_ptr().cast());
    let job = handle(unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) })?;
    let mut limits: Limits = unsafe { std::mem::zeroed() };
    limits.basic.flags = if kill_on_close { 0x2000 } else { 0 };
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            9,
            (&mut limits as *mut Limits).cast(),
            std::mem::size_of::<Limits>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut broker_attrs = Attributes::new(1)?;
    let mut jobs = [job.as_raw_handle()];
    broker_attrs.set(0x2000d, &mut jobs)?;
    let mut broker_start: StartupEx = unsafe { std::mem::zeroed() };
    broker_start.startup.cb = std::mem::size_of::<StartupEx>() as u32;
    broker_start.attributes = broker_attrs.ptr();
    let mut broker_info: ProcessInfo = unsafe { std::mem::zeroed() };
    let mut broker_line = wide(broker_line)?;
    if unsafe {
        CreateProcessW(
            app.as_ptr(),
            broker_line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            0x80000 | 0x400 | 4 | 0x08000000,
            env_ptr,
            cwd.as_ref().map_or(std::ptr::null(), |x| x.as_ptr()),
            &mut broker_start.startup,
            &mut broker_info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let broker = Broker {
        process: handle(broker_info.process)?,
        thread: handle(broker_info.thread)?,
        resume_on_drop: !kill_on_close,
    };
    let (child_in, parent_in) = pipe()?;
    let (parent_out, child_out) = pipe()?;
    let (parent_err, child_err) = pipe()?;
    for h in [&parent_in, &parent_out, &parent_err] {
        non_inheritable(h)?;
    }
    let mut remote_in = RemoteHandle::new(&child_in, &broker.process)?;
    let mut remote_out = RemoteHandle::new(&child_out, &broker.process)?;
    let mut remote_err = RemoteHandle::new(&child_err, &broker.process)?;
    #[cfg(feature = "test-support")]
    {
        let observer = {
            *SPAWN_OBSERVER
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        };
        if let Some(observer) = observer {
            observer();
        }
    }
    let mut attrs = Attributes::new(2)?;
    let mut parents = [broker.process.as_raw_handle()];
    attrs.set(0x20000, &mut parents)?;
    let mut inherited = [remote_in.raw, remote_out.raw, remote_err.raw];
    attrs.set(0x20002, &mut inherited)?;
    let mut startup: StartupEx = unsafe { std::mem::zeroed() };
    startup.startup.cb = std::mem::size_of::<StartupEx>() as u32;
    startup.startup.flags = 0x100;
    startup.startup.stdin = inherited[0];
    startup.startup.stdout = inherited[1];
    startup.startup.stderr = inherited[2];
    startup.attributes = attrs.ptr();
    let mut info: ProcessInfo = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            0x80000 | 0x400 | 0x08000000,
            env_ptr,
            cwd.as_ref().map_or(std::ptr::null(), |x| x.as_ptr()),
            &mut startup.startup,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let process = handle(info.process)?;
    let _thread = handle(info.thread)?;
    remote_in.close()?;
    remote_out.close()?;
    remote_err.close()?;
    drop((remote_in, remote_out, remote_err));
    let process = NativeProcess::new(process, job, broker, kill_on_close);
    drop((child_in, child_out, child_err));
    let stdin = tokio::process::ChildStdin::from_std(std::process::ChildStdin::from(parent_in))?;
    let stdout =
        tokio::process::ChildStdout::from_std(std::process::ChildStdout::from(parent_out))?;
    let stderr =
        tokio::process::ChildStderr::from_std(std::process::ChildStderr::from(parent_err))?;
    Ok(Spawned {
        process,
        stdin,
        stdout,
        stderr,
    })
}
