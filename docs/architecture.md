# Architecture

## Design principle

**All price data is local.** zonk reads exclusively from warehouse parquet files. No Yahoo Finance, no external price APIs. Universe membership is resolved from local preset JSON files. The only external HTTP call is the optional CBOE VIX CSV download (cached for 24h).

## Why the split

The `market-data-warehouse` repo handles data ingestion, storage, and warehouse management. The `zonk` crate is the standalone home for all quantitative strategy research and backtesting. It reads from the shared `~/market-warehouse/` data lake but has zero dependency on the warehouse repo.

## Data contract

zonk reads from the bronze parquet layer:

```
~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet
```

Expected schema per parquet file:
- `trade_date` (date/datetime)
- `open`, `high`, `low`, `close` (float)
- `volume` (int)
- `adj_close` (float) — equal to `close` in this warehouse (IB TRADES data, split-adjusted but not dividend-adjusted)

## Data flow

1. **Universe resolution**: Tickers loaded from `presets/<name>.json` (static membership, no API)
2. **Trading calendar**: Derived from lead forward asset's parquet date column
3. **Constituent prices**: Loaded via `load_price_panel()` from warehouse parquet
4. **Forward-return prices**: Same — loaded from warehouse parquet
5. **Computation**: SMA, breadth, forward returns, risk metrics — all in-process
6. **Output**: CSV, JSON, and `_viz.json` files written to `output/`

A 10-year backtest across 101 tickers completes in ~0.3 seconds on Apple Silicon.

## Universe membership

Universe membership is resolved locally from preset JSON files:

| Universe | Preset file | Tickers |
|----------|-------------|---------|
| `ndx100` | `presets/ndx100.json` | 101 |
| `sp500` | `presets/sp500.json` | ~500 |
| `r2k` | `presets/r2k.json` | ~2000 |
| `all-stocks` | warehouse dir scan | ~7000+ |

Custom presets can be created with the format: `{ "name": "my-universe", "tickers": ["AAPL", "MSFT", ...] }`

## Crate layout

```
src/
├── main.rs                          # Binary entrypoint (includes execution timer)
├── lib.rs                           # Library root (re-exports all modules)
├── cli.rs                           # Unified CLI: zonk run <strategy> | list-strategies | list-presets
├── config.rs                        # Centralized config: warehouse path, output root, presets dir
├── data/
│   ├── mod.rs
│   ├── paths.rs                     # Parquet path resolution helpers
│   ├── discovery.rs                 # Symbol discovery from bronze layer
│   ├── readers.rs                   # Parquet (polars) data loaders, load_price_panel(), CBOE VIX cache
│   └── presets.rs                   # Preset loading + validation
├── metrics/
│   ├── mod.rs
│   ├── performance.rs               # cagr, sharpe, max_drawdown, var_95, annual_returns_table
│   └── fees.rs                      # IBKR fee model constants + ibkr_roundtrip_cost()
└── strategies/
    ├── mod.rs
    ├── common.rs                    # Shared: daily_returns, buy_and_hold_equity, formatting, JSON output
    ├── overnight_drift.rs           # Buy close, sell next open; optional VIX filter + ADF test
    ├── intraday_drift.rs            # Buy open, sell close same day; long or short
    ├── breadth_washout.rs           # Generic breadth signal across any universe
    ├── breadth_ma.rs                # Single MA breadth (default: 50-day)
    ├── breadth_dual_ma.rs           # Dual MA breadth; close < short MA AND close > long MA
    ├── ndx100_sma_breadth.rs        # NDX-100 SMA breadth analysis + forward returns
    └── ndx100_breadth_washout.rs    # Thin wrapper
```

## Key dependencies

- `polars` — DataFrame & parquet I/O
- `nalgebra` — Linear algebra for ADF test OLS
- `clap` — CLI argument parsing with derive macros, global `--output` flag
- `serde` + `serde_json` — JSON serialization for `--output json` mode
- `reqwest` — HTTP client (CBOE VIX download only; no price data fetching)
- `chrono` — Date/time operations

## Config precedence

```
ZONK_WAREHOUSE_PATH env var → .env file → ~/market-warehouse (default)
```

## Output modes

All strategies support `--output text` (default) and `--output json`. The flag is global on the `Cli` struct and passed to each strategy's `run(args, fmt)` function. When `json`, strategies emit a single JSON object to stdout with no progress text.

## How to add a new strategy

1. Create `src/strategies/my_strategy.rs`
2. Define `MyStrategyArgs` struct using clap derive
3. Implement `pub fn run(args: &MyStrategyArgs, fmt: OutputFormat) -> Result<()>`
4. Load prices with `crate::data::readers::load_price_panel()` — never fetch from external APIs
5. Load universe membership from preset JSON or CLI tickers — never call external membership APIs
6. Add the strategy to `StrategyCommand` enum in `src/cli.rs`
7. Wire it up in `src/main.rs` match arm
8. Write tests in the `#[cfg(test)] mod tests` block
9. Run `cargo test`
