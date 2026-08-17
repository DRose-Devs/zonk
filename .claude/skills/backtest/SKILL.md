# Backtest

Run a doob paper-research backtest with standard parameters.

## Usage
```
/backtest <asset> <rule> [extra flags]
```

## Instructions

1. Parse the user's arguments. Expected format: `<ASSET> <RULE> [--flag value ...]`
   - ASSET: ticker symbol (e.g. SPY, QQQ, TQQQ)
   - RULE: one of `trend_momentum`, `trend_pullback`, `rsi_reversion`, `volatility_regime`, `vol_spread`
   - Extra flags are passed through directly

2. Build the doob binary if needed:
   ```bash
   cargo build --release 2>&1 | tail -3
   ```

3. Run the backtest with standard date windows:
   ```bash
   ./target/release/doob --output json run paper-research --asset <ASSET> --rule <RULE> \
     --end-date 2024-12-31 --sessions 1260 <extra_flags>
   ```

4. Parse the JSON output and display a summary table:
   - Strategy name, CAGR, Sharpe, Max Drawdown, Final Equity
   - Compare against Buy & Hold baseline

5. If the user didn't specify parameter flags, use sensible defaults:
   - trend_momentum/trend_pullback: `--fast-window 12 --slow-window 50`
   - rsi_reversion: `--fast-window 12 --slow-window 50 --rsi-window 14 --rsi-oversold 28 --rsi-overbought 72`
   - volatility_regime: `--fast-window 12 --slow-window 50 --vol-window 20 --vol-cap 0.40`
   - vol_spread: `--fast-window 12 --slow-window 50 --vol-window 22 --vol-cap 0.20`
