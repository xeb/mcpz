#!/bin/bash

# Test script for mcpz package detection
# Tests various MCP packages across different registries

set -e

MCPZ="./target/release/mcpz"
PASSED=0
FAILED=0
TOTAL=0

# Build mcpz first
echo "Building mcpz..."
cargo build --release 2>/dev/null

# Clear cache before testing
echo "Clearing cache..."
$MCPZ clear-cache 2>/dev/null || true

# Test function - checks if mcpz can detect the package
test_package() {
    local package="$1"
    local expected_type="$2"  # "cargo", "python", or "npm"

    TOTAL=$((TOTAL + 1))
    echo -n "Testing '$package' (expected: $expected_type)... "

    # Run search and capture output
    output=$($MCPZ search "$package" 2>&1) || true

    case "$expected_type" in
        cargo)
            if echo "$output" | grep -q "Found.*crates.io"; then
                echo "✓ PASS"
                PASSED=$((PASSED + 1))
                return 0
            fi
            ;;
        python)
            if echo "$output" | grep -q "Found.*PyPI"; then
                echo "✓ PASS"
                PASSED=$((PASSED + 1))
                return 0
            fi
            ;;
        npm)
            if echo "$output" | grep -q "Found.*npm"; then
                echo "✓ PASS"
                PASSED=$((PASSED + 1))
                return 0
            fi
            ;;
    esac

    echo "✗ FAIL"
    FAILED=$((FAILED + 1))
    return 1
}

echo ""
echo "=========================================="
echo "Testing MCP Package Detection"
echo "=========================================="
echo ""

# Python/PyPI packages (should be found with uvx)
echo "--- Python/PyPI Packages ---"
test_package "mcp-server-time" "python" || true
test_package "mcp-server-fetch" "python" || true
test_package "mcp-server-sqlite" "python" || true
test_package "mcp-server-filesystem" "python" || true

echo ""
echo "--- npm Packages ---"
test_package "@modelcontextprotocol/server-filesystem" "npm" || true
test_package "@modelcontextprotocol/server-memory" "npm" || true
test_package "@modelcontextprotocol/server-sequential-thinking" "npm" || true
test_package "@modelcontextprotocol/sdk" "npm" || true

echo ""
echo "--- Cargo Packages ---"
test_package "ripgrep" "cargo" || true
test_package "fd-find" "cargo" || true

echo ""
echo "=========================================="
echo "Results: $PASSED/$TOTAL passed, $FAILED failed"
echo "=========================================="

if [ $FAILED -eq 0 ]; then
    echo ""
    echo "☑️  Tests Passed"
    exit 0
else
    echo ""
    echo "❌ Some tests failed"
    exit 1
fi
