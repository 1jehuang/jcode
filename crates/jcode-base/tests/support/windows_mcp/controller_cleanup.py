def attempt_all(actions):
    errors = []
    for name, action in actions:
        try:
            action()
        except BaseException as error:
            errors.append(f"{name}: {type(error).__name__}: {error}")
    return errors


def cleanup_fixture(kernel, job, info, attr, ready, control, receipt, out, err):
    def close(handle):
        if handle and not kernel.CloseHandle(handle):
            raise RuntimeError("CloseHandle returned zero")

    def wait_fixture():
        if info.process and kernel.WaitForSingleObject(info.process, 10000) != 0:
            raise RuntimeError("contained test did not exit after owned Job closure")

    def finish_control():
        if control is None:
            return
        if control.poll() is not None:
            raise RuntimeError("outside control exited before verification")
        control.communicate(input=b"x", timeout=10)
        if control.returncode:
            raise RuntimeError("outside control failed natural exit")
        receipt["outside_control_verified"] = True

    def close_control_stdin():
        if control is not None and control.stdin is not None:
            control.stdin.close()

    def wait_control():
        if control is not None and control.wait(timeout=10):
            raise RuntimeError("outside control exit was not successful")

    return attempt_all([
        ("close_owned_job", lambda: close(job)),
        ("close_thread", lambda: close(info.thread)),
        ("wait_test_exit", wait_fixture),
        ("close_test_handle", lambda: close(info.process)),
        ("delete_attributes", lambda: kernel.DeleteProcThreadAttributeList(attr) if ready else None),
        ("verify_outside_control", finish_control),
        ("close_control_stdin", close_control_stdin),
        ("wait_control_exit", wait_control),
        ("flush_stdout", out.flush),
        ("flush_stderr", err.flush),
    ])
