import ctypes as c
from ctypes import wintypes as w
import msvcrt, os, subprocess, sys, tempfile
from pathlib import Path
from contextlib import nullcontext
import hashlib, json, re
from windows_mcp_fixture import create_job
from windows_joblist_prototype import Startup, StartupEx, ProcessInfo
from controller_cleanup import cleanup_fixture


def verify_one_test(output, name):
    running = re.findall(r"^running (\d+) tests?$", output, re.MULTILINE)
    summaries = re.findall(r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;", output, re.MULTILINE)
    named = re.search(r"^test " + re.escape(name) + r" \.\.\.", output, re.MULTILINE)
    if running != ["1"] or len(summaries) != 1 or summaries[0][:4] != ("1", "0", "0", "0") or not named:
        raise ValueError("expected exactly one selected passing test and zero failures")


def main():
    exe = Path(sys.argv[1]).resolve(strict=True)
    args = sys.argv[2:]
    if not args or not re.fullmatch(r"[A-Za-z0-9_:]+", args[0]):
        raise ValueError("provide one exact test name")
    if not {"--exact", "--ignored", "--nocapture", "--test-threads=1"}.issubset(args):
        raise ValueError("exact ignored nocapture and test-threads=1 are required")
    fixture = Path(__file__).with_name('windows_mcp_fixture.py').resolve(strict=True)
    k, job = create_job()
    k.SetHandleInformation.argtypes = [w.HANDLE, w.DWORD, w.DWORD]
    k.InitializeProcThreadAttributeList.argtypes = [c.c_void_p, w.DWORD, w.DWORD, c.POINTER(c.c_size_t)]
    k.UpdateProcThreadAttribute.argtypes = [c.c_void_p, w.DWORD, c.c_size_t, c.c_void_p, c.c_size_t, c.c_void_p, c.c_void_p]
    k.DeleteProcThreadAttributeList.argtypes = [c.c_void_p]
    k.CreateProcessW.argtypes = [w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p, w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(Startup), c.POINTER(ProcessInfo)]
    k.IsProcessInJob.argtypes = [w.HANDLE, w.HANDLE, c.POINTER(w.BOOL)]
    k.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]
    k.ResumeThread.argtypes = [w.HANDLE]
    k.ResumeThread.restype = w.DWORD
    k.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    k.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
    k.CloseHandle.argtypes = [w.HANDLE]
    info = ProcessInfo()
    attr = None
    ready = False
    control = None
    td = tempfile.mkdtemp(prefix="jcode-mcp-controller-")
    print(f"MCP_CONTROLLER_ARTIFACTS {td}", flush=True)
    receipt_path = Path(td) / "receipt.json"
    receipt = {"binary": str(exe), "binary_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
               "controller_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(), "args": args,
               "support_sha256": {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                                  for path in Path(__file__).parent.glob("*.py")},
               "selected_test_verified": False, "outside_control_verified": False,
               "controller_success": False, "phase": "prepared"}
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    with nullcontext(td) as td:
        env = {n: os.environ[n] for n in ('SYSTEMROOT','WINDIR','PATH','PATHEXT','COMSPEC') if n in os.environ}
        env.update({n: td for n in ('HOME','USERPROFILE','JCODE_HOME','TEMP','TMP','APPDATA','LOCALAPPDATA')})
        env.update(JCODE_MCP_TEST_CONTROLLER='1', JCODE_MCP_TEST_FIXTURE=str(fixture), JCODE_MCP_TEST_PYTHON=sys.executable)
        with open(Path(td)/'stdout.log','w+b') as out, open(Path(td)/'stderr.log','w+b') as err, open(os.devnull,'rb') as inp:
            try:
                control = subprocess.Popen([sys.executable, '-c', 'import sys;sys.stdin.read(1)'], stdin=subprocess.PIPE, env=env, cwd=td)
                in_job=w.BOOL()
                if not k.IsProcessInJob(int(control._handle), job, c.byref(in_job)) or in_job.value:
                    raise RuntimeError('control not proven outside job')
                if not k.SetHandleInformation(job, 1, 0): raise c.WinError(c.get_last_error())
                raw=[msvcrt.get_osfhandle(x.fileno()) for x in (inp,out,err)]
                for h in raw:
                    if not k.SetHandleInformation(h,1,1): raise c.WinError(c.get_last_error())
                size=c.c_size_t(); k.InitializeProcThreadAttributeList(None,2,0,c.byref(size))
                buf=c.create_string_buffer(size.value); attr=c.cast(buf,c.c_void_p)
                if not k.InitializeProcThreadAttributeList(attr,2,0,c.byref(size)): raise c.WinError(c.get_last_error())
                ready=True
                jobs=(w.HANDLE*1)(job); std=(w.HANDLE*3)(*raw)
                for key, value in ((0x2000d,jobs),(0x20002,std)):
                    if not k.UpdateProcThreadAttribute(attr,0,key,c.byref(value),c.sizeof(value),None,None): raise c.WinError(c.get_last_error())
                start=StartupEx(); start.startup.cb=c.sizeof(start); start.startup.flags=0x100
                start.startup.stdin,start.startup.stdout,start.startup.stderr=raw; start.attributes=attr
                cmd=c.create_unicode_buffer(subprocess.list2cmdline([str(exe),*sys.argv[2:]]))
                envblock=c.create_unicode_buffer('\0'.join(f'{key}={value}' for key,value in sorted(env.items(),key=lambda pair:pair[0].upper()))+'\0\0')
                if not k.CreateProcessW(str(exe),cmd,None,None,True,0x80000|0x400|4|0x08000000,c.cast(envblock,c.c_void_p),td,c.byref(start.startup),c.byref(info)): raise c.WinError(c.get_last_error())
                inside=w.BOOL(); size=w.DWORD(32768); path=c.create_unicode_buffer(size.value)
                if not k.IsProcessInJob(info.process,job,c.byref(inside)) or not inside.value: raise RuntimeError('test process not proven inside job')
                if not k.QueryFullProcessImageNameW(info.process,0,path,c.byref(size)) or os.path.normcase(path.value)!=os.path.normcase(str(exe)): raise RuntimeError('test process image mismatch')
                if k.ResumeThread(info.thread)!=1: raise RuntimeError('unexpected suspended thread state')
                wait=k.WaitForSingleObject(info.process,180000)
                if wait!=0: raise RuntimeError(f'test process wait failed or timed out: {wait}')
                code=w.DWORD()
                if not k.GetExitCodeProcess(info.process,c.byref(code)): raise c.WinError(c.get_last_error())
                receipt["process_exit"] = code.value
                out.flush(); err.flush(); out.seek(0); err.seek(0)
                output = out.read().decode(errors='replace')
                print(output); print(err.read().decode(errors='replace'),file=sys.stderr)
                if code.value: raise RuntimeError(f'test process failed: {code.value}')
                verify_one_test(output, args[0])
                receipt["selected_test_verified"] = True
            except BaseException as error:
                receipt["error"] = f"{type(error).__name__}: {error}"
                raise
            finally:
                cleanup_errors = cleanup_fixture(k, job, info, attr, ready, control, receipt, out, err)
                receipt["phase"] = "cleanup_attempted"
                receipt["cleanup_errors"] = cleanup_errors
                try:
                    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
                except BaseException as error:
                    cleanup_errors.append(f"write_receipt: {type(error).__name__}: {error}")
                if cleanup_errors:
                    raise RuntimeError(f"cleanup failed after all actions attempted: {cleanup_errors}")
    receipt["controller_success"] = receipt["selected_test_verified"] and receipt["outside_control_verified"]
    receipt["phase"] = "complete"
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    if not receipt["controller_success"]:
        raise RuntimeError(f"controller validation failed; durable evidence: {td}")
    print('PASS: exactly one selected test passed; outside-job control verified separately')

if __name__ == '__main__': main()
