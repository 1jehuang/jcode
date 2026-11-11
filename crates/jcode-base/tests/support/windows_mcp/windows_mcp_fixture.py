import ctypes
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def process_identity(handle):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    from ctypes import wintypes
    kernel.GetProcessTimes.argtypes = [ctypes.c_void_p] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.QueryFullProcessImageNameW.argtypes = [ctypes.c_void_p, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    created, exited, kernel_time, user_time = (wintypes.FILETIME() for _ in range(4))
    if not kernel.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel_time), ctypes.byref(user_time)):
        raise ctypes.WinError(ctypes.get_last_error())
    size = wintypes.DWORD(32768)
    image = ctypes.create_unicode_buffer(size.value)
    if not kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    return {"creation_time": (created.dwHighDateTime << 32) | created.dwLowDateTime, "image": image.value}


def serve(record, job=None):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetCurrentProcess.restype = ctypes.c_void_p
    kernel.AssignProcessToJobObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    if job is not None:
        if not kernel.AssignProcessToJobObject(int(job), kernel.GetCurrentProcess()):
            raise ctypes.WinError(ctypes.get_last_error())
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        kernel.CloseHandle(int(job))
    else:
        kernel.IsProcessInJob.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_int)]
        inside = ctypes.c_int()
        if not kernel.IsProcessInJob(kernel.GetCurrentProcess(), None, ctypes.byref(inside)) or not inside.value:
            raise RuntimeError("fixture must run inside the controller job")
    child = subprocess.Popen([sys.executable, __file__, "grandchild"])
    with open(record, "a", encoding="utf-8") as out:
        out.write(json.dumps({"wrapper": os.getpid(), "grandchild": child.pid,
                              "wrapper_identity": process_identity(kernel.GetCurrentProcess()),
                              "grandchild_identity": process_identity(int(child._handle))}) + "\n")
    for line in sys.stdin:
        request = json.loads(line)
        method = request.get("method")
        if method == "shutdown":
            return
        if method == "initialize":
            if "fail-init" in sys.argv:
                signal = Path(sys.argv[sys.argv.index("fail-init") + 1])
                while not signal.exists():
                    time.sleep(0.02)
                print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32603, "message": "synthetic init failure"}}), flush=True)
                continue
            result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": []}
        else:
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
        if method == "tools/list" and "block-after-init" in sys.argv:
            time.sleep(300)


def create_job():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    class BasicLimit(ctypes.Structure):
        _fields_ = [("user", ctypes.c_int64), ("job_user", ctypes.c_int64), ("flags", ctypes.c_uint32), ("min_working", ctypes.c_size_t), ("max_working", ctypes.c_size_t), ("active", ctypes.c_uint32), ("affinity", ctypes.c_size_t), ("priority", ctypes.c_uint32), ("scheduling", ctypes.c_uint32)]
    class IoCounters(ctypes.Structure):
        _fields_ = [(name, ctypes.c_uint64) for name in ("reads", "writes", "other", "read_bytes", "write_bytes", "other_bytes")]
    class ExtendedLimit(ctypes.Structure):
        _fields_ = [("basic", BasicLimit), ("io", IoCounters), ("process_memory", ctypes.c_size_t), ("job_memory", ctypes.c_size_t), ("peak_process", ctypes.c_size_t), ("peak_job", ctypes.c_size_t)]
    kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p]
    kernel.CreateJobObjectW.restype = ctypes.c_void_p
    kernel.SetInformationJobObject.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    kernel.SetHandleInformation.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    job = kernel.CreateJobObjectW(None, None)
    if not job:
        raise ctypes.WinError(ctypes.get_last_error())
    limits = ExtendedLimit()
    limits.basic.flags = 0x2000
    if not kernel.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits)) or not kernel.SetHandleInformation(job, 1, 1):
        kernel.CloseHandle(job)
        raise ctypes.WinError(ctypes.get_last_error())
    return kernel, job


if __name__ == "__main__":
    if sys.argv[1] == "grandchild":
        time.sleep(300)
    elif sys.argv[1] == "serve-in-job":
        serve(sys.argv[2])
    else:
        raise ValueError("unsupported dedicated fixture mode")
