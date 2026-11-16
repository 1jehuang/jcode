"""Keep platform-specific bash test hooks out of Windows test builds."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BashTestPlatformGuards(unittest.TestCase):
    def test_launch_gate_uses_the_test_modules_platform_guard(self):
        source = (ROOT / "crates/jcode-app-core/src/tool/bash.rs").read_text()
        module = re.search(
            r"#\[cfg\(([^\n]+)\)\]\s*"
            r'#\[path = "bash_tests.rs"\]\s*mod tests;', source
        )
        hook = re.search(
            r"#\[cfg\(([^\n]+)\)\]\s*"
            r"tests::wait_for_session_env_launch_gate\(&session_id_env\)\.await;",
            source,
        )
        self.assertIsNotNone(module)
        self.assertIsNotNone(hook)
        self.assertEqual(module.group(1), "all(test, not(windows))")
        self.assertEqual(hook.group(1), module.group(1))


if __name__ == "__main__":
    unittest.main()
