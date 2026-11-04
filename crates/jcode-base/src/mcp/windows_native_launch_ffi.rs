#[repr(C)]
struct Security {
    length: u32,
    descriptor: Handle,
    inherit: i32,
}
#[repr(C)]
struct Startup {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    xs: u32,
    ys: u32,
    xc: u32,
    yc: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2: u16,
    reserved_ptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}
#[repr(C)]
struct StartupEx {
    startup: Startup,
    attributes: Handle,
}
#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}
#[repr(C)]
struct BasicLimits {
    user: i64,
    job_user: i64,
    flags: u32,
    min: usize,
    max: usize,
    active: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct IoCounters {
    reads: u64,
    writes: u64,
    other: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}
#[repr(C)]
struct Limits {
    basic: BasicLimits,
    io: IoCounters,
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFullPathNameW(path: *const u16, size: u32, buffer: *mut u16, file: *mut *mut u16) -> u32;
    fn CreateJobObjectW(attrs: Handle, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: Handle, size: u32) -> i32;
    fn CreatePipe(read: *mut Handle, write: *mut Handle, attrs: *mut Security, size: u32) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn InitializeProcThreadAttributeList(
        attrs: Handle,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        attrs: Handle,
        flags: u32,
        key: usize,
        value: Handle,
        size: usize,
        previous: Handle,
        returned: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(attrs: Handle);
    fn CreateProcessW(
        app: *const u16,
        line: *mut u16,
        process_attrs: Handle,
        thread_attrs: Handle,
        inherit: i32,
        flags: u32,
        env: Handle,
        cwd: *const u16,
        startup: *mut Startup,
        info: *mut ProcessInfo,
    ) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source: Handle,
        handle: Handle,
        target: Handle,
        duplicate: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn ResumeThread(thread: Handle) -> u32;
}
