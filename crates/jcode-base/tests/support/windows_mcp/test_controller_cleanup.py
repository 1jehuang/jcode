from types import SimpleNamespace
import unittest

from controller_cleanup import attempt_all, cleanup_fixture


class CleanupFailures(unittest.TestCase):
    def fixture(self, wait_fails=False, flush_fails=False):
        calls = []

        class Kernel:
            def CloseHandle(self, handle):
                calls.append(f"close:{handle}")
                return 1

            def WaitForSingleObject(self, handle, timeout):
                calls.append(f"wait:{handle}")
                return 258 if wait_fails else 0

            def DeleteProcThreadAttributeList(self, attr):
                calls.append("delete_attributes")

        class Control:
            returncode = None
            stdin = SimpleNamespace(close=lambda: calls.append("close_control_stdin"))

            def poll(self):
                return None

            def communicate(self, input, timeout):
                calls.append("communicate_control")
                self.returncode = 0

            def wait(self, timeout):
                calls.append("wait_control")
                return 0

        class Stream:
            def flush(self):
                calls.append("flush")
                if flush_fails:
                    raise OSError("injected disk failure")

        receipt = {"outside_control_verified": False}
        errors = cleanup_fixture(
            Kernel(), "job", SimpleNamespace(thread="thread", process="process"),
            "attr", True, Control(), receipt, Stream(), Stream()
        )
        return calls, errors, receipt

    def test_flush_failure_occurs_after_all_resource_cleanup(self):
        calls, errors, receipt = self.fixture(flush_fails=True)
        self.assertEqual(calls[:3], ["close:job", "close:thread", "wait:process"])
        self.assertLess(calls.index("wait_control"), calls.index("flush"))
        self.assertIn("close:process", calls)
        self.assertTrue(receipt["outside_control_verified"])
        self.assertEqual(len(errors), 2)

    def test_wait_failure_does_not_skip_other_cleanup(self):
        calls, errors, receipt = self.fixture(wait_fails=True)
        self.assertIn("close:process", calls)
        self.assertIn("delete_attributes", calls)
        self.assertIn("communicate_control", calls)
        self.assertIn("wait_control", calls)
        self.assertTrue(receipt["outside_control_verified"])
        self.assertEqual(len(errors), 1)

    def test_each_failure_still_attempts_later_actions(self):
        for failure in range(4):
            calls = []

            def action(index):
                calls.append(index)
                if index == failure:
                    raise OSError("injected cleanup failure")

            errors = attempt_all([(str(index), lambda index=index: action(index)) for index in range(4)])
            self.assertEqual(calls, [0, 1, 2, 3])
            self.assertEqual(len(errors), 1)

    def test_receipt_failure_cannot_skip_preceding_cleanup(self):
        calls = []

        def fail_receipt():
            calls.append("receipt")
            raise OSError("injected receipt failure")

        errors = attempt_all([
            ("job", lambda: calls.append("job")),
            ("control", lambda: calls.append("control")),
            ("receipt", fail_receipt),
        ])
        self.assertEqual(calls, ["job", "control", "receipt"])
        self.assertEqual(len(errors), 1)


if __name__ == "__main__":
    unittest.main()
