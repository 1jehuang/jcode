#!/usr/bin/env python3
"""Regression tests for production safety-budget classification."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import contextlib
import io


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ProductionLinesTests(unittest.TestCase):
    def test_cfg_test_qualified_functions_do_not_leak_into_production(self):
        for name in ("check_panic_budget", "check_swallowed_error_budget"):
            module = load(name)
            for signature in ("async fn", "pub(super) async fn", "unsafe fn", "const fn", "fn"):
                with self.subTest(scanner=name, signature=signature), tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "source.rs"
                    path.write_text("\n".join([
                        "#[cfg(test)]", "#[allow(dead_code)]",
                        f"{signature} helper() {{",
                        "    nested({ dangerous.unwrap(); ignored.ok(); });", "}",
                        "async fn production() { real.expect(\"required\"); real.ok(); }",
                    ]))
                    lines = "\n".join(module.production_lines(path))
                    self.assertNotIn("dangerous", lines)
                    self.assertIn("real.expect", lines)
                    self.assertIn("real.ok", lines)

    def test_regressions_still_fail_without_writing_baselines(self):
        cases = [
            ("check_panic_budget", {"src/old.rs": 1}, {"src/old.rs": 2}),
            ("check_panic_budget", {"src/old.rs": 2}, {"src/new.rs": 1}),
            ("check_swallowed_error_budget", {"src/old.rs": {"dot_ok": 1}}, {"src/old.rs": {"dot_ok": 2}}),
            ("check_swallowed_error_budget", {"src/old.rs": {"dot_ok": 2}}, {"src/new.rs": {"dot_ok": 1}}),
        ]
        for name, old, current in cases:
            module = load(name)
            baseline = {"version": 1, "tracked_files": old}
            if name == "check_panic_budget":
                baseline["total"] = sum(old.values())
            else:
                baseline["total"] = module.grand_total(old)
                baseline["totals_by_pattern"] = module.total_counts(old)
            with self.subTest(scanner=name, current=current), mock.patch.object(module, "parse_args", return_value=type("Args", (), {"update": False})()), mock.patch.object(module, "load_baseline", return_value=baseline), mock.patch.object(module, "current_counts", return_value=current), mock.patch.object(module, "write_baseline") as write, contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(module.main(), 1)
                write.assert_not_called()

    def test_inline_cfg_and_multiline_signature_do_not_hide_next_production_item(self):
        for name in ("check_panic_budget", "check_swallowed_error_budget"):
            module = load(name)
            with self.subTest(scanner=name), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "source.rs"
                path.write_text('#[cfg(test)] fn helper() { fake.unwrap(); fake.ok(); }\n#[cfg(test)]\npub async fn multiline(\n    value: usize,\n) { fake.unwrap(); fake.ok(); }\nfn production() { real.unwrap(); real.ok(); }\n')
                text = "\n".join(module.production_lines(path))
                self.assertNotIn("fake", text)
                self.assertIn("real.unwrap()", text)
                self.assertIn("real.ok()", text)

    def test_literals_and_comments_do_not_count_or_unbalance_test_blocks(self):
        source = '''// fake.unwrap(); fake.ok(); {
/* nested /* fake.expect("bad"); */ fake.ok(); } */
const TEXT: &str = r##"fake.unwrap(); fake.ok(); {"##;
#[cfg(test)]
mod tests {
    const C: char = '}';
    fn helper() { fake.unwrap(); fake.ok(); }
}
fn real<'a>(value: &'a str) { real.unwrap(); real.ok(); }
'''
        for name in ("check_panic_budget", "check_swallowed_error_budget"):
            module = load(name)
            with self.subTest(scanner=name), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "source.rs"
                path.write_text(source)
                text = "\n".join(module.production_lines(path))
                self.assertNotIn("fake", text)
                self.assertIn("real.unwrap()", text)
                self.assertIn("real.ok()", text)

    def test_external_test_modules_paths_nested_and_shared_references(self):
        for name in ("check_panic_budget", "check_swallowed_error_budget"):
            module = load(name)
            with self.subTest(scanner=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "src").mkdir()
                sources = {
                    "lib.rs": '#[cfg(test)]\n#[path = "renamed.rs"]\nmod checks;\nmod production;\n// #[cfg(test)] mod production;\nconst TEXT: &str = r#"mod production;"#;',
                    "renamed.rs": 'mod nested;\n#[path = "shared.rs"]\nmod shared;',
                    "renamed/nested.rs": 'fn helper() { fake.unwrap(); fake.ok(); }',
                    "production.rs": '#[path = "shared.rs"]\nmod shared;\nfn real() { real.unwrap(); real.ok(); }',
                    "shared.rs": 'fn real_shared() { real.unwrap(); real.ok(); }',
                }
                for relative, source in sources.items():
                    path = root / "src" / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(source)
                with mock.patch.object(module, "REPO_ROOT", root), mock.patch.object(module, "SCAN_ROOTS", (root / "src",)):
                    files = {path.relative_to(root / "src").as_posix() for path in module.production_rust_files()}
                self.assertNotIn("renamed.rs", files)
                self.assertNotIn("renamed/nested.rs", files)
                self.assertIn("production.rs", files)
                self.assertIn("shared.rs", files)

    def test_non_test_cfg_and_unconditional_async_remain_counted(self):
        for name in ("check_panic_budget", "check_swallowed_error_budget"):
            module = load(name)
            with self.subTest(scanner=name), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "source.rs"
                path.write_text('#[cfg(unix)]\npub async fn production() { value.unwrap(); value.ok(); }\n')
                self.assertIn("value.unwrap", "\n".join(module.production_lines(path)))


if __name__ == "__main__":
    unittest.main()
