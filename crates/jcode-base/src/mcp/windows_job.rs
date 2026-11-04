use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, GetProcessIdOfThread, OpenThread, ResumeThread,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, WaitForSingleObject,
};

fn owned(raw: HANDLE) -> io::Result<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}

pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, OwnedHandle)> {
    use std::os::windows::io::AsRawHandle;

    let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    command.creation_flags(CREATE_SUSPENDED).kill_on_drop(true);
    let mut child = command.spawn()?;
    let result = (|| {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("MCP child has no process handle"))?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("MCP child has no process ID"))?;
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut thread_id = None;
        loop {
            if entry.th32OwnerProcessID == pid {
                if thread_id.replace(entry.th32ThreadID).is_some() {
                    return Err(io::Error::other("suspended MCP child has multiple threads"));
                }
            }
            if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(18) {
                    return Err(error);
                }
                break;
            }
        }
        let thread = owned(unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                0,
                thread_id.ok_or_else(|| io::Error::other("suspended MCP thread not found"))?,
            )
        })?;
        if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != pid
            || unsafe { WaitForSingleObject(process, 0) } != 258
        {
            return Err(io::Error::other("suspended MCP process identity changed"));
        }
        if unsafe { ResumeThread(thread.as_raw_handle()) } != 1 {
            return Err(io::Error::other(
                "could not safely resume suspended MCP thread",
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = child.start_kill();
        return Err(error);
    }
    Ok((child, job))
}
