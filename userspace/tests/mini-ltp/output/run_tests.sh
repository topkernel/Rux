#!/bin/sh
# Mini LTP Test Runner Script

TEST_DIR=/test/mini-ltp/bin
PASSED=0
FAILED=0
SKIPPED=0

echo "========================================"
echo "Rux OS Kernel Compatibility Tests"
echo "========================================"
echo ""

run_test() {
    test_name=$1
    if [ -x "$TEST_DIR/$test_name" ]; then
        echo -n "Testing $test_name... "
        if "$TEST_DIR/$test_name" > /dev/null 2>&1; then
            echo "PASS"
            PASSED=$((PASSED + 1))
        else
            echo "FAIL"
            FAILED=$((FAILED + 1))
        fi
    else
        SKIPPED=$((SKIPPED + 1))
    fi
}

# Run all tests
for test in "$TEST_DIR"/*; do
    if [ -x "$test" ]; then
        name=$(basename "$test")
        run_test "$name"
    fi
done

echo ""
echo "========================================"
echo "Results: $PASSED passed, $FAILED failed, $SKIPPED skipped"
echo "========================================"

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
