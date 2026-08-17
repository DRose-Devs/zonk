# Quant Backtesting Framework in Rust

## Core principles

- **All price data is local.** Every strategy reads from `~/market-warehouse/` bronze parquet via `load_price_panel()`. No Yahoo Finance, no external price APIs.
- **Universe membership is local.** Resolved from `presets/<name>.json` files. No NASDAQ API, no external membership lookups.
- **Only external call**: Optional CBOE VIX CSV download (cached 24h), used only by `overnight-drift` strategy.

## Strategy notes

- The breadth strategy entry point is `zonk run breadth-washout`.
- It supports `oversold` and `overbought` trigger modes across named universes, custom presets, explicit ticker lists, and `all-stocks`.
- All universes (ndx100, sp500, r2k) resolve from local preset JSON files as static membership baskets.
- Data is read from the shared `~/market-warehouse/` bronze parquet layer.
- ADF test uses pure Rust nalgebra OLS with AIC lag selection (no external stats dependency).
- Fee model: IBKR tiered with per-share commission, exchange/regulatory fees, min order, and max cap.
- All strategies support `--output json` for programmatic/agent consumption (single JSON object to stdout, no progress text).

## Performance

A 10-year, 101-ticker backtest with full risk metrics (Sharpe, Sortino, max drawdown, VaR, CVaR, profit factor) runs in ~0.3 seconds on Apple Silicon.

See the full backtesting framework reference in the parent market-data-warehouse repo's `docs/backtesting.md`.

## Automated candidate search

- `src/bin/autoresearch_loop.rs` provides a Rust-native auto-research loop for generating candidates, running train/test backtests, scoring, and ranking strategies.
- Use `--seed-web` to pull arXiv-inspired candidate ideas via Exa API (`EXA_API_KEY`).
- Use `--verbose` for detailed per-candidate command and metric output.
- Best candidate details are reported with command args and summary metrics; full run rows append to `reports/autoresearch-ledger.jsonl`.
