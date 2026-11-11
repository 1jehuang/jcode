import unittest
from atomic_controller import verify_one_test


class SummaryValidation(unittest.TestCase):
    def output(self, name="expected", running=1, passed=1, failed=0, ignored=0):
        return (f"running {running} tests\n" f"test {name} ... fixture output\nok\n"
                f"test result: ok. {passed} passed; {failed} failed; {ignored} ignored; "
                "0 measured; 7 filtered out; finished in 0.1s\n")

    def test_selected_success(self):
        verify_one_test(self.output(), "expected")

    def test_zero_tests_cannot_pass(self):
        with self.assertRaises(ValueError):
            verify_one_test(self.output(running=0, passed=0), "expected")

    def test_wrong_selected_test_cannot_pass(self):
        with self.assertRaises(ValueError):
            verify_one_test(self.output(name="other"), "expected")

    def test_failure_cannot_pass(self):
        with self.assertRaises(ValueError):
            verify_one_test(self.output(failed=1), "expected")

    def test_multiple_tests_cannot_pass(self):
        with self.assertRaises(ValueError):
            verify_one_test(self.output(running=2, passed=2), "expected")

    def test_ignored_test_cannot_pass(self):
        with self.assertRaises(ValueError):
            verify_one_test(self.output(passed=0, ignored=1), "expected")


if __name__ == "__main__":
    unittest.main()
