#!/usr/bin/env bash
#
# CLI integration test suite for zonk
# Exercises every command, flag combination, and output format.
# Uses real warehouse data at ~/market-warehouse.
#
# Usage: ./tests/cli_integration.sh
# Exit code 0 = all pass, 1 = failures

set -euo pipefail

ZONK="./target/release/zonk"
PASS=0
FAIL=0
ERRORS=""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

run_test() {
    local name="$1"
    shift
    local cmd="$*"

    printf "  %-60s " "$name"
    if output=$(eval "$cmd" 2>&1); then
        printf "${GREEN}PASS${NC}\n"
        PASS=$((PASS + 1))
    else
        printf "${RED}FAIL${NC}\n"
        FAIL=$((FAIL + 1))
        ERRORS="${ERRORS}\n  ${RED}✗${NC} $name\n    cmd: $cmd\n    out: $(echo "$output" | tail -3)\n"
    fi
}

# Expect failure (non-zero exit)
run_test_fail() {
    local name="$1"
    shift
    local cmd="$*"

    printf "  %-60s " "$name"
    if output=$(eval "$cmd" 2>&1); then
        printf "${RED}FAIL (expected error but succeeded)${NC}\n"
        FAIL=$((FAIL + 1))
        ERRORS="${ERRORS}\n  ${RED}✗${NC} $name (expected failure)\n    cmd: $cmd\n"
    else
        printf "${GREEN}PASS (expected error)${NC}\n"
        PASS=$((PASS + 1))
    fi
}

# Check output contains a string
run_test_grep() {
    local name="$1"
    local pattern="$2"
    shift 2
    local cmd="$*"

    printf "  %-60s " "$name"
    if output=$(eval "$cmd" 2>&1) && echo "$output" | grep -q "$pattern"; then
        printf "${GREEN}PASS${NC}\n"
        PASS=$((PASS + 1))
    else
        printf "${RED}FAIL${NC}\n"
        FAIL=$((FAIL + 1))
        ERRORS="${ERRORS}\n  ${RED}✗${NC} $name\n    pattern: $pattern\n    cmd: $cmd\n    out: $(echo "$output" | tail -3)\n"
    fi
}

echo ""
echo "========================================"
echo "  zonk CLI Integration Tests"
echo "========================================"
echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── Top-level commands ──${NC}"
# ---------------------------------------------------------------

run_test "help flag" "$ZONK --help"
run_test "list-strategies" "$ZONK list-strategies"
run_test_grep "list-strategies contains overnight-drift" "overnight-drift" "$ZONK list-strategies"
run_test_grep "list-strategies contains breadth-washout" "breadth-washout" "$ZONK list-strategies"
run_test_grep "list-strategies contains breadth-ma" "breadth-ma" "$ZONK list-strategies"
run_test_grep "list-strategies contains breadth-dual-ma" "breadth-dual-ma" "$ZONK list-strategies"
run_test "list-presets" "$ZONK list-presets"
run_test_grep "list-presets contains ndx100" "ndx100" "$ZONK list-presets"
run_test_grep "list-presets contains sp500" "sp500" "$ZONK list-presets"
run_test "list-strategies --output json" "$ZONK --output json list-strategies"
run_test "list-presets --output json" "$ZONK --output json list-presets"
run_test "run --help" "$ZONK run --help"
run_test_fail "no subcommand errors" "$ZONK run"
run_test_fail "invalid strategy errors" "$ZONK run nonexistent-strategy"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── overnight-drift ──${NC}"
# ---------------------------------------------------------------

run_test "overnight-drift defaults" \
    "$ZONK run overnight-drift --no-plots --no-vix-filter"
run_test "overnight-drift --output json" \
    "$ZONK --output json run overnight-drift --no-plots --no-vix-filter"
run_test_grep "overnight-drift output contains CAGR" "CAGR" \
    "$ZONK run overnight-drift --no-plots --no-vix-filter"
run_test "overnight-drift with dates" \
    "$ZONK run overnight-drift --no-plots --no-vix-filter --start-date 2020-01-01 --end-date 2023-12-31"
run_test "overnight-drift custom capital" \
    "$ZONK run overnight-drift --no-plots --no-vix-filter --capital 500000"
run_test "overnight-drift with vix filter" \
    "$ZONK run overnight-drift --no-plots"
run_test "overnight-drift custom start-year-table" \
    "$ZONK run overnight-drift --no-plots --no-vix-filter --start-year-table 2020"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── intraday-drift ──${NC}"
# ---------------------------------------------------------------

run_test "intraday-drift defaults (SPY long)" \
    "$ZONK run intraday-drift --no-plots"
run_test "intraday-drift SPY short" \
    "$ZONK run intraday-drift --no-plots --short"
run_test "intraday-drift QQQ" \
    "$ZONK run intraday-drift --no-plots --ticker QQQ"
run_test "intraday-drift QQQ short" \
    "$ZONK run intraday-drift --no-plots --ticker QQQ --short"
run_test "intraday-drift with dates" \
    "$ZONK run intraday-drift --no-plots --start-date 2020-01-01 --end-date 2023-12-31"
run_test "intraday-drift custom capital" \
    "$ZONK run intraday-drift --no-plots --capital 500000"
run_test "intraday-drift --output json" \
    "$ZONK --output json run intraday-drift --no-plots"
run_test "intraday-drift custom start-year-table" \
    "$ZONK run intraday-drift --no-plots --start-year-table 2020"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── breadth-washout ──${NC}"
# ---------------------------------------------------------------

run_test "breadth-washout defaults (ndx100 oversold)" \
    "$ZONK run breadth-washout"
run_test "breadth-washout --output json" \
    "$ZONK --output json run breadth-washout"
run_test "breadth-washout custom threshold" \
    "$ZONK run breadth-washout --threshold 70"
run_test "breadth-washout min-pct-below alias" \
    "$ZONK run breadth-washout --min-pct-below 80"
run_test "breadth-washout overbought mode" \
    "$ZONK run breadth-washout --signal-mode overbought --threshold 70"
run_test "breadth-washout lookback 10" \
    "$ZONK run breadth-washout --lookback 10"
run_test "breadth-washout 50 sessions" \
    "$ZONK run breadth-washout --sessions 50"
run_test "breadth-washout custom assets" \
    "$ZONK run breadth-washout --assets QQQ TQQQ"
run_test "breadth-washout explicit tickers" \
    "$ZONK run breadth-washout --tickers AAPL MSFT GOOGL AMZN META --threshold 50"
run_test "breadth-washout explicit tickers + label" \
    "$ZONK run breadth-washout --tickers AAPL MSFT GOOGL --universe-label mega-tech --threshold 50"
run_test "breadth-washout universe sp500" \
    "$ZONK run breadth-washout --universe sp500"
run_test "breadth-washout universe r2k" \
    "$ZONK run breadth-washout --universe r2k --sessions 50"
run_test "breadth-washout price-returns flag" \
    "$ZONK run breadth-washout --price-returns"
run_test "breadth-washout custom end-date" \
    "$ZONK run breadth-washout --end-date 2025-12-31"
run_test "breadth-washout custom horizon" \
    "$ZONK run breadth-washout --horizon 2w=10"
run_test_fail "breadth-washout invalid signal mode" \
    "$ZONK run breadth-washout --signal-mode invalid"
run_test_fail "breadth-washout invalid universe" \
    "$ZONK run breadth-washout --universe nonexistent"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── breadth-ma ──${NC}"
# ---------------------------------------------------------------

run_test "breadth-ma defaults (50d oversold 80%)" \
    "$ZONK run breadth-ma"
run_test "breadth-ma --output json" \
    "$ZONK --output json run breadth-ma"
run_test "breadth-ma 20-day MA" \
    "$ZONK run breadth-ma --short-period 20"
run_test "breadth-ma 100-day MA" \
    "$ZONK run breadth-ma --short-period 100"
run_test "breadth-ma custom threshold" \
    "$ZONK run breadth-ma --threshold 60"
run_test "breadth-ma overbought" \
    "$ZONK run breadth-ma --signal-mode overbought --threshold 60"
run_test "breadth-ma custom assets" \
    "$ZONK run breadth-ma --assets QQQ TQQQ"
run_test "breadth-ma explicit tickers" \
    "$ZONK run breadth-ma --tickers AAPL MSFT GOOGL AMZN META --threshold 50"
run_test "breadth-ma sp500" \
    "$ZONK run breadth-ma --universe sp500 --threshold 60 --sessions 50"
run_test "breadth-ma 50 sessions" \
    "$ZONK run breadth-ma --sessions 50 --threshold 50"
run_test "breadth-ma price-returns" \
    "$ZONK run breadth-ma --price-returns"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── breadth-dual-ma ──${NC}"
# ---------------------------------------------------------------

run_test "breadth-dual-ma defaults (50d/200d 20%)" \
    "$ZONK run breadth-dual-ma"
run_test "breadth-dual-ma --output json" \
    "$ZONK --output json run breadth-dual-ma"
run_test "breadth-dual-ma custom periods" \
    "$ZONK run breadth-dual-ma --short-period 20 --long-period 100 --threshold 30"
run_test "breadth-dual-ma low threshold" \
    "$ZONK run breadth-dual-ma --threshold 10"
run_test "breadth-dual-ma custom assets" \
    "$ZONK run breadth-dual-ma --assets QQQ TQQQ"
run_test "breadth-dual-ma explicit tickers" \
    "$ZONK run breadth-dual-ma --tickers AAPL MSFT GOOGL AMZN META --threshold 10"
run_test "breadth-dual-ma sp500" \
    "$ZONK run breadth-dual-ma --universe sp500 --threshold 10 --sessions 50"
run_test "breadth-dual-ma 50 sessions" \
    "$ZONK run breadth-dual-ma --sessions 50 --threshold 10"
run_test "breadth-dual-ma price-returns" \
    "$ZONK run breadth-dual-ma --price-returns"
run_test_fail "breadth-dual-ma short >= long period" \
    "$ZONK run breadth-dual-ma --short-period 200 --long-period 50"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── ndx100-sma-breadth ──${NC}"
# ---------------------------------------------------------------

run_test "ndx100-sma-breadth defaults" \
    "$ZONK run ndx100-sma-breadth"
run_test "ndx100-sma-breadth --output json" \
    "$ZONK --output json run ndx100-sma-breadth"
run_test "ndx100-sma-breadth custom sessions" \
    "$ZONK run ndx100-sma-breadth --sessions 50"
run_test "ndx100-sma-breadth custom lookback" \
    "$ZONK run ndx100-sma-breadth --lookback 10"
run_test "ndx100-sma-breadth custom end-date" \
    "$ZONK run ndx100-sma-breadth --end-date 2025-12-31"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── ndx100-breadth-washout ──${NC}"
# ---------------------------------------------------------------

run_test "ndx100-breadth-washout defaults" \
    "$ZONK run ndx100-breadth-washout"
run_test "ndx100-breadth-washout --output json" \
    "$ZONK --output json run ndx100-breadth-washout"
run_test "ndx100-breadth-washout custom threshold" \
    "$ZONK run ndx100-breadth-washout --threshold 70"
run_test "ndx100-breadth-washout custom assets" \
    "$ZONK run ndx100-breadth-washout --assets QQQ TQQQ"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── Output format combinations ──${NC}"
# ---------------------------------------------------------------

run_test "global --output text (explicit)" \
    "$ZONK --output text run overnight-drift --no-plots --no-vix-filter"
run_test "global --output json overnight-drift" \
    "$ZONK --output json run overnight-drift --no-plots --no-vix-filter"
run_test "global --output json intraday-drift" \
    "$ZONK --output json run intraday-drift --no-plots"
run_test "global --output json breadth-washout" \
    "$ZONK --output json run breadth-washout"
run_test "global --output json breadth-ma" \
    "$ZONK --output json run breadth-ma"
run_test "global --output json breadth-dual-ma" \
    "$ZONK --output json run breadth-dual-ma"
run_test "global --output json ndx100-sma-breadth" \
    "$ZONK --output json run ndx100-sma-breadth"
run_test "global --output md overnight-drift" \
    "$ZONK --output md run overnight-drift --no-plots --no-vix-filter"
run_test "global --output md intraday-drift" \
    "$ZONK --output md run intraday-drift --no-plots"
run_test "global --output md breadth-washout" \
    "$ZONK --output md run breadth-washout"
run_test "global --output md breadth-ma" \
    "$ZONK --output md run breadth-ma"
run_test "global --output md breadth-dual-ma" \
    "$ZONK --output md run breadth-dual-ma"
run_test "global --output md ndx100-sma-breadth" \
    "$ZONK --output md run ndx100-sma-breadth"
run_test_fail "invalid output format" \
    "$ZONK --output csv run overnight-drift --no-plots"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── JSON output validation ──${NC}"
# ---------------------------------------------------------------

run_test_grep "json overnight-drift is valid JSON" "strategy" \
    "$ZONK --output json run overnight-drift --no-plots --no-vix-filter | python3 -m json.tool > /dev/null && echo strategy"
run_test_grep "json intraday-drift is valid JSON" "strategy" \
    "$ZONK --output json run intraday-drift --no-plots | python3 -m json.tool > /dev/null && echo strategy"
run_test_grep "json breadth-washout is valid JSON" "strategy" \
    "$ZONK --output json run breadth-washout | python3 -m json.tool > /dev/null && echo strategy"
run_test_grep "json breadth-ma is valid JSON" "strategy" \
    "$ZONK --output json run breadth-ma | python3 -m json.tool > /dev/null && echo strategy"
run_test_grep "json breadth-dual-ma is valid JSON" "strategy" \
    "$ZONK --output json run breadth-dual-ma | python3 -m json.tool > /dev/null && echo strategy"
run_test_grep "json ndx100-sma-breadth is valid JSON" "strategy" \
    "$ZONK --output json run ndx100-sma-breadth | python3 -m json.tool > /dev/null && echo strategy"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── Markdown output validation ──${NC}"
# ---------------------------------------------------------------

run_test_grep "md overnight-drift has h1" "^# " \
    "$ZONK --output md run overnight-drift --no-plots --no-vix-filter"
run_test_grep "md overnight-drift has table" "|---" \
    "$ZONK --output md run overnight-drift --no-plots --no-vix-filter"
run_test_grep "md breadth-washout has h1" "^# " \
    "$ZONK --output md run breadth-washout"
run_test_grep "md breadth-washout has Forward Returns table" "Forward Returns" \
    "$ZONK --output md run breadth-washout"
run_test_grep "md breadth-dual-ma has Risk Metrics" "Risk Metrics" \
    "$ZONK --output md run breadth-dual-ma"
run_test_grep "md ndx100-sma-breadth has Distribution" "Distribution" \
    "$ZONK --output md run ndx100-sma-breadth"
run_test_grep "md has no log noise" "^#" \
    "$ZONK --output md run breadth-washout | head -1"

echo ""

# ---------------------------------------------------------------
echo "${YELLOW}── Edge cases ──${NC}"
# ---------------------------------------------------------------

run_test_fail "breadth-washout future end-date" \
    "$ZONK run breadth-washout --end-date 2030-01-01"
run_test_fail "breadth-washout 0 sessions" \
    "$ZONK run breadth-washout --sessions 0"
run_test_fail "breadth-washout missing ticker in warehouse" \
    "$ZONK run breadth-washout --tickers ZZZZNOTREAL --threshold 0"

echo ""

# ---------------------------------------------------------------
# Summary
# ---------------------------------------------------------------

TOTAL=$((PASS + FAIL))
echo "========================================"
if [ $FAIL -eq 0 ]; then
    printf "  ${GREEN}ALL $TOTAL TESTS PASSED${NC}\n"
else
    printf "  ${RED}$FAIL / $TOTAL TESTS FAILED${NC}\n"
    printf "\nFailures:$ERRORS\n"
fi
echo "========================================"
echo ""

exit $FAIL
