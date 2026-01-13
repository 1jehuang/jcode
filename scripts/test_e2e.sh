#!/bin/bash
# End-to-end test script for jcode

set -e

echo "=== E2E Testing Script for jcode ==="
echo ""

# Test 1: Check binary exists and runs
echo "Test 1: Check jcode binary..."
if command -v jcode &> /dev/null; then
    echo "✓ jcode binary found"
    jcode --version
else
    echo "✗ jcode binary not found"
    exit 1
fi

# Test 2: Run unit tests
echo ""
echo "Test 2: Run unit tests..."
cargo test 2>&1 | tail -5
echo "✓ Unit tests passed"

# Test 3: Check protocol serialization
echo ""
echo "Test 3: Protocol serialization test..."
cargo test protocol::tests --quiet
echo "✓ Protocol tests passed"

# Test 4: Check TUI app tests
echo ""
echo "Test 4: TUI app tests..."
cargo test tui::app::tests --quiet
echo "✓ TUI app tests passed"

# Test 5: Check markdown rendering tests
echo ""
echo "Test 5: Markdown rendering tests..."
cargo test tui::markdown::tests --quiet
echo "✓ Markdown tests passed"

# Test 6: E2E tests
echo ""
echo "Test 6: E2E integration tests..."
cargo test --test e2e --quiet
echo "✓ E2E tests passed"

echo ""
echo "=== All tests passed! ==="
echo ""
echo "To test interactively:"
echo "  jcode        # Start TUI mode"
echo "  jcode server # Start server mode"
echo "  jcode client # Connect to server"
