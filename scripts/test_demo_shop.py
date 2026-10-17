#!/usr/bin/env python3
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("demo_shop.py")
LAUNCHER = Path(__file__).with_name("launch_agentcard_discovery_demo.sh")
SPEC = importlib.util.spec_from_file_location("demo_shop", SCRIPT)
assert SPEC and SPEC.loader
shop = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = shop
SPEC.loader.exec_module(shop)

class DemoShopTests(unittest.TestCase):
    def run_cli(self, *args: str, state: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            capture_output=True,
            text=True,
            env={**os.environ, "KCODE_DEMO_SHOP_STATE": str(state)},
            timeout=10,
        )

    def test_search_filters_to_charger_under_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_cli(
                "search",
                "65W",
                "USB-C",
                "charger",
                "--max-price",
                "50",
                state=Path(directory) / "state.json",
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("charger-65w", result.stdout)
        self.assertNotIn("charger-100w", result.stdout)

    def test_checkout_exposes_payment_gap_without_ordering(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "state.json"
            added = self.run_cli("add", "charger-65w", state=state)
            checkout = self.run_cli("checkout", state=state)
        self.assertEqual(added.returncode, 0, added.stderr)
        self.assertEqual(checkout.returncode, 3)
        self.assertIn("Checkout total: $43.19", checkout.stdout)
        self.assertIn("no payment method is available", checkout.stdout)
        self.assertIn("stop before creating or funding a prepaid card", checkout.stdout.lower())
        self.assertIn("No account was created", checkout.stdout)
        self.assertNotIn("capability", checkout.stdout.lower())
        self.assertNotIn("setup instructions", checkout.stdout.lower())
        self.assertNotIn("agentcard", checkout.stdout.lower())
        self.assertNotIn("discover_tools", checkout.stdout.lower())

    def test_prepare_checkout_reaches_gap_in_one_command(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "state.json"
            result = self.run_cli(
                "prepare-checkout",
                "charger-65w",
                "--max-total",
                "50",
                state=state,
            )
        self.assertEqual(result.returncode, 3)
        self.assertIn("Checkout total: $43.19", result.stdout)
        self.assertIn("no payment method is available", result.stdout)

    def test_prepare_checkout_enforces_total_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_cli(
                "prepare-checkout",
                "charger-100w",
                "--max-total",
                "50",
                state=Path(directory) / "state.json",
            )
        self.assertEqual(result.returncode, 2)
        self.assertIn("exceeds limit", result.stderr)

    def test_reset_clears_cart(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "state.json"
            self.run_cli("add", "charger-65w", state=state)
            reset = self.run_cli("reset", state=state)
            cart = self.run_cli("cart", state=state)
        self.assertEqual(reset.returncode, 0)
        self.assertIn("Cart is empty", reset.stdout)
        self.assertIn("Demo cart is empty", cart.stdout)

    def test_totals_are_deterministic(self):
        self.assertEqual(shop.totals(["charger-65w"]), (39.99, 0.0, 3.2, 43.19))

    def test_launcher_prompt_names_neither_target_tool_nor_discovery(self):
        prompt_line = next(
            line
            for line in LAUNCHER.read_text(encoding="utf-8").splitlines()
            if line.startswith("PROMPT=")
        )
        lowered = prompt_line.lower()
        self.assertIn("Use `./bin/jcode-demo-shop`", prompt_line)
        self.assertIn("USB-C laptop charger", prompt_line)
        self.assertIn("work through any prerequisites", lowered)
        self.assertIn("ask me for confirmation immediately before", lowered)
        self.assertIn("prepaid card", lowered)
        self.assertNotIn("agentcard", lowered)
        self.assertNotIn("discover_tools", lowered)
        self.assertNotIn("discovery", lowered)
        self.assertNotIn("missing capability", lowered)
        self.assertNotIn("setup instructions", lowered)
        self.assertNotIn("prepare-checkout", lowered)
        self.assertNotIn("charger-65w", lowered)


if __name__ == "__main__":
    unittest.main()
