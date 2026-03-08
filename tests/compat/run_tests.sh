#!/bin/bash
# Run C tup test suite against the Rust binary.
#
# Usage:
#   ./tests/compat/run_tests.sh              # Run default test set (t0000-t1009)
#   ./tests/compat/run_tests.sh t0000-init   # Run a specific test
#   ./tests/compat/run_tests.sh t0*.sh       # Run tests matching a pattern
#
# Prerequisites:
#   - C tup test suite at ~/git/tup/test/
#   - Rust binary built via `cargo build`

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
C_TEST_DIR="$HOME/git/tup/test"

if [ ! -d "$C_TEST_DIR" ]; then
    echo "Error: C tup test suite not found at $C_TEST_DIR" >&2
    exit 1
fi

# Build the Rust binary
echo "Building tup-rust..."
cd "$PROJECT_ROOT"
cargo build 2>&1 | tail -1

# Find the binary
TUP_BIN="$PROJECT_ROOT/target/debug/tup"
if [ ! -x "$TUP_BIN" ]; then
    echo "Error: tup binary not found at $TUP_BIN" >&2
    exit 1
fi

# Create directory structure that tup.sh expects:
#   PARENT_DIR/tup          <- our binary (symlink)
#   PARENT_DIR/test/        <- test scripts run from here
#
# tup.sh sets PATH=$PWD/..:$PATH, so from test/ it finds PARENT_DIR/tup
PARENT_DIR=$(mktemp -d /tmp/tup-compat-XXXXXX)
WORK_DIR="$PARENT_DIR/test"
mkdir "$WORK_DIR"
trap "rm -rf $PARENT_DIR" EXIT

# Link our binary where tup.sh will find it
ln -s "$TUP_BIN" "$PARENT_DIR/tup"

# Copy test helper and testTupfile
cp "$C_TEST_DIR/tup.sh" "$WORK_DIR/"
if [ -f "$C_TEST_DIR/testTupfile.tup" ]; then
    cp "$C_TEST_DIR/testTupfile.tup" "$WORK_DIR/"
fi

# Determine which tests to run
if [ $# -gt 0 ]; then
    TESTS=()
    for arg in "$@"; do
        # Support short names like "t0000-init" or full names like "t0000-init.sh"
        if [[ "$arg" == *.sh ]]; then
            pattern="$arg"
        else
            pattern="${arg}*.sh"
        fi
        for f in "$C_TEST_DIR"/$pattern; do
            if [ -f "$f" ]; then
                TESTS+=("$f")
            fi
        done
    done
else
    # Default: run t0000-t0005 and t1000-t1009
    TESTS=()
    for f in "$C_TEST_DIR"/t000[0-5]*.sh "$C_TEST_DIR"/t100[0-9]*.sh; do
        if [ -f "$f" ]; then
            TESTS+=("$f")
        fi
    done
fi

if [ ${#TESTS[@]} -eq 0 ]; then
    echo "No tests found matching the given pattern." >&2
    exit 1
fi

echo ""
echo "Running ${#TESTS[@]} test(s) against tup-rust..."
echo "Binary: $TUP_BIN"
echo "Tests:  $C_TEST_DIR"
echo ""

PASSED=0
FAILED=0
SKIPPED=0
FAILURES=""

for test_file in "${TESTS[@]}"; do
    test_name=$(basename "$test_file" .sh)

    # Copy the test to the work directory
    cp "$test_file" "$WORK_DIR/"

    # Run the test from the work directory (so tup.sh's PATH setup works)
    cd "$WORK_DIR"

    printf "  %-40s " "$test_name"

    # Run with sh -e, capture output
    if output=$(sh -e "$(basename "$test_file")" 2>&1); then
        echo "PASS"
        PASSED=$((PASSED + 1))
    else
        rc=$?
        if echo "$output" | grep -q "Skipping test"; then
            echo "SKIP"
            SKIPPED=$((SKIPPED + 1))
        else
            echo "FAIL (exit $rc)"
            FAILURES="$FAILURES  $test_name: exit $rc\n"
            if [ -n "$VERBOSE" ]; then
                echo "    Output:"
                echo "$output" | sed 's/^/    /'
            fi
            FAILED=$((FAILED + 1))
        fi
    fi

    # Clean up any test temp directories
    rm -rf "$WORK_DIR"/tuptesttmp-* 2>/dev/null || true
done

echo ""
echo "Results: $PASSED passed, $FAILED failed, $SKIPPED skipped"

if [ $FAILED -gt 0 ]; then
    echo ""
    echo "Failures:"
    printf "$FAILURES"
    exit 1
fi
