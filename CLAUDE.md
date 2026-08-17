# zonk — Quantitative Strategy Research & Backtesting

Rust binary for quantitative strategy research. Reads from the shared `~/market-warehouse/` data lake (bronze parquet). **All price data comes from local parquet files — no Yahoo Finance or external API calls for price data.** Universe membership is resolved from local preset JSON files (e.g. `presets/ndx100.json`). The only external HTTP call is the optional CBOE VIX CSV download (cached for 24h). VIX OHLCV is also available from local parquet at `asset_class=volatility/symbol=VIX`.

## Autoresearch Loop (Mandatory Rust-Only Path)

- Use `cargo build --release` (or `cargo build` for debug) to build the Rust binaries.
- Run autoresearch with the Rust binary, not Python:
  - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - verbose trace:
    - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - default output now includes strategy category, candidate assets, horizon, and rationale summary in the results table.
  - **production mode is paper-research only**: arXiv/Exa-hypothesis-driven loops are executed as `paper-research` strategies.
  - `cargo run --release --bin autoresearch_loop -- --zonk-bin target/release/zonk --seed-web --train-start 2020-01-01 --train-end 2024-12-31 --test-start 2025-01-01 --test-end 2026-03-11 --train-sessions 1008 --test-sessions 252`
- **Iterative refinement** is the default (`--max-rounds 10`). After round 0 exploration, the loop refines top winners by perturbing parameters within the existing discrete grids and swapping assets. Convergence stops the loop when no meaningful improvement is found (`--patience 3`, `--min-improvement 0.02`). Use `--no-loop` for legacy single-pass behavior.
  - `--refine-top 5` — number of winners to refine per round
  - `--refine-variants 30` — max variants generated per winner
- **Asset universe expansion** controls which assets are tested during refinement rounds:
  - `--asset-universe core` — 5 core assets (SPY, QQQ, SPXL, IWM, TQQQ) — fastest, backward-compatible
  - `--asset-universe broad` — SP500 + NDX100 + core (~550 tickers) — default
  - `--asset-universe full` — all viable warehouse symbols (~3,500+, adds ~10s startup for parquet metadata scan)
  - `--asset-universe <preset>` — any preset name (e.g. `ndx100`, `sp500`)
  - `--refine-asset-swaps 10` — max asset swap variants per winner per refinement round
  - `--min-asset-rows <N>` — minimum parquet rows for asset viability (default: auto = train+test sessions)
  - Round 0 exploration always uses core assets (5) for speed; expanded universe only applies to refinement rounds
- **Quality gates** filter the final report to only "investable" strategies:
  - `--min-sharpe 1.0` — reject strategies with test-window Sharpe below threshold
  - `--max-drawdown 20` — reject strategies with test-window drawdown worse than -20%
  - When gates are active, the report shows how many candidates pass. If none pass, unfiltered results are shown for reference
- **Convergence summary** prints a diagnostic when the refinement loop terminates, including:
  - Stop reason (patience exhausted, frontier exhausted, max rounds reached)
  - Total evaluated/passed counts, exhausted refinement centers
  - Rule and asset distribution among passing candidates
  - Score trajectory from first to last round
- **Evaluation cache** (`reports/autoresearch-eval-cache.jsonl`) persists results across runs, keyed by parameter signature + date windows. Deterministic grid candidates and repeated seeded params are served instantly from cache. Use `--no-cache` to force re-evaluation.
- Results are appended to `reports/autoresearch-ledger.jsonl` and `reports/autoresearch-exa-ideas.json`.
- Persisted top-10 winners must also write train/test audit JSON under `reports/autoresearch-audits/`, including exact actual periods, trade ledgers, and equity traces.
- Promoted winners are upserted into `reports/autoresearch-strategy-registry.json`, keyed by stable parameter signature, so future implementation work can reuse audited strategies instead of scraping JSONL history.
- Interactive report output: `reports/autoresearch-top10-interactive-report.html`.
- Exa seeding uses `EXA_API_KEY` from environment; keep python script usage out of the autoresearch path.
- Use `.env.example` as a starter file:
  - `cp .env.example .env`
  - `EXA_API_KEY=your_exa_api_key_here`
- Then run:
  - `printf 'EXA_API_KEY=%s\\n' \"$EXA_API_KEY\" > .env`
  - `cargo run --release --bin autoresearch_loop -- --seed-web`

## Crate Layout

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
│   ├── readers.rs                   # Parquet (polars) data loaders, load_price_panel(), load_vix_ohlcv(), load_volatility_index_ohlcv(), CBOE VIX cache
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
    ├── breadth_washout.rs           # Generic breadth signal across any universe (default: 5-day SMA)
    ├── breadth_ma.rs                # Single MA breadth (default: 50-day); % below/above N-day MA
    ├── breadth_dual_ma.rs           # Dual MA breadth; close < short MA AND close > long MA
    ├── ndx100_sma_breadth.rs        # NDX-100 SMA breadth analysis + forward returns
    └── ndx100_breadth_washout.rs    # Thin wrapper
```

## Data Architecture

**All price data is read from local warehouse parquet files.** No Yahoo Finance, no external price APIs.

- Price data: `~/market-warehouse/data-lake/bronze/asset_class=equity/symbol=<TICKER>/data.parquet`
- VIX data (parquet): `~/market-warehouse/data-lake/bronze/asset_class=volatility/symbol=VIX/data.parquet`
- VVIX data (parquet): `~/market-warehouse/data-lake/bronze/asset_class=volatility/symbol=VVIX/data.parquet`
- VIX data (CBOE CSV): cached locally for 24h (only external HTTP call, used by overnight-drift)
- Universe membership: `presets/<universe>.json` (e.g. `presets/ndx100.json`, `presets/sp500.json`)

Expected parquet columns: `trade_date`, `open`, `high`, `low`, `close`, `volume`.

**Important**: `adj_close == close` in this warehouse (IB TRADES data is split-adjusted but not dividend-adjusted). Buy-and-hold CAGR will understate true total return by ~1.3%/yr due to missing dividends.

### Data flow

1. Universe tickers loaded from `presets/<name>.json` (static membership, no API)
2. Price data for each ticker loaded from warehouse parquet via `load_price_panel()`
3. Forward-return asset prices (QQQ, TQQQ, SPY, etc.) also from warehouse parquet
4. Trading calendar derived from lead forward asset's parquet dates
5. All computation is local — a 10-year, 101-ticker backtest runs in ~0.3 seconds

## Config Precedence

`ZONK_WAREHOUSE_PATH` env var → `.env` file → `~/market-warehouse` (default)

## Strategy Catalog

```bash
zonk run overnight-drift --no-vix-filter --no-plots
zonk run intraday-drift --ticker SPY --short
zonk run paper-research --output json --asset TQQQ --rule rsi_reversion --fast-window 16 --slow-window 18 --rsi-window 16 --rsi-oversold 28 --rsi-overbought 76 --vol-window 20 --vol-cap 0.40
zonk run paper-research --asset SPY --rule vol_spread --vol-window 22 --vol-cap 0.20
zonk run paper-research --asset QQQ --rule mean_reversion_filter --slow-window 50 --mr-entry-threshold 0.02
zonk run paper-research --asset SPXL --rule vvix_regime --vvix-window 42 --vvix-threshold 0.50 --vvix-mode risk_off
zonk run breadth-washout --universe ndx100 --signal-mode oversold
zonk run breadth-washout --universe ndx100 --lookback 50 --signal-mode oversold --threshold 80
zonk run ndx100-sma-breadth --end-date 2026-03-11
zonk run breadth-ma --universe ndx100 --short-period 50 --signal-mode oversold --threshold 80
zonk run breadth-dual-ma --universe ndx100 --short-period 50 --long-period 200 --threshold 20
zonk list-strategies
zonk list-presets
```

### breadth-washout

Generic breadth signal strategy. Computes % of a universe above/below an N-day SMA
(`--lookback`, default 5). Triggers oversold/overbought at a threshold. Computes
forward returns on configurable assets (SPY, SPXL, QQQ, TQQQ, etc.) with full
risk metrics (Sharpe, Sortino, max drawdown, VaR, CVaR, profit factor).

Key flags: `--lookback 5` (SMA period), `--threshold 65` (trigger level),
`--signal-mode oversold|overbought`, `--assets QQQ TQQQ`, `--sessions 5831`.

Universe membership is loaded from preset JSON files. All price data comes from
local warehouse parquet — no network calls.

Output: `_summary.csv`, `_triggers.csv`, `_membership_changes.csv`, `.json` meta,
`_viz.json` (dashboard consumption). Max drawdown in output is the compounded equity
curve drawdown (not worst single trade).

### breadth-ma

Breadth strategy using a single configurable MA period (default 50-day). Computes
% of universe below/above the N-day MA and triggers signals at a threshold. Same
forward-return and risk-metric pipeline as breadth-washout. Supports the same
universe modes (ndx100, sp500, r2k, all-stocks, preset, tickers).

### breadth-dual-ma

Dual moving-average breadth strategy. For each stock, checks TWO conditions:
`close < short-period MA AND close > long-period MA`. This identifies stocks in
a short-term pullback while still in a long-term uptrend. Computes the % of the
universe meeting both conditions simultaneously, and triggers signals when that
% crosses a threshold.

### paper-research

Single-asset signal strategy used by the autoresearch loop. Applies one of five
signal rules to close-price history and evaluates a long-only equity curve against
a buy-and-hold baseline. Rules:

| Rule | Signal | Key Params |
|------|--------|------------|
| `trend_momentum` | Long when fast MA > slow MA and price > fast MA | `--fast-window`, `--slow-window` |
| `trend_pullback` | Long when price < fast MA but > slow MA | `--fast-window`, `--slow-window` |
| `rsi_reversion` | Long when RSI < oversold threshold | `--rsi-window`, `--rsi-oversold` |
| `volatility_regime` | Long when realized vol below percentile cap | `--vol-window`, `--vol-cap [0,1]` |
| `vol_spread` | Long when VIX-implied vs realized vol spread exceeds threshold | `--vol-window`, `--vol-cap` (negative = snap-back) |
| `mean_reversion_filter` | Long when price drops below fair-value SMA by threshold | `--slow-window`, `--mr-entry-threshold`, `--mr-scale` |
| `vvix_regime` | Long based on VVIX percentile rank (risk_off or contrarian) | `--vvix-window`, `--vvix-threshold`, `--vvix-mode` |

The `vol_spread` rule is the first multi-source signal — it loads VIX from local
parquet (`asset_class=volatility/symbol=VIX`) and compares implied vol against
realized vol annualized with `sqrt(252)`. Positive `--vol-cap` = VRP harvest
(VIX overstates realized); negative = snap-back (realized overshoots implied).

The `mean_reversion_filter` rule implements Xu et al. (2026, SSRN 6225198) —
computes fair value as SMA(close, slow_window), then relative mispricing
δ = (close − SMA) / SMA. Goes long when δ < −mr_entry_threshold. The SMA is a
simplified proxy for the paper's LAFO-trained neural signal filters.

The `vvix_regime` rule implements Bevilacqua & Hizmeri (2026, SSRN 6212458) —
loads VVIX from local parquet (`asset_class=volatility/symbol=VVIX`) via
`load_volatility_index_ohlcv()` and computes rolling percentile rank. In
`risk_off` mode: long when VVIX is below threshold (calm conditions). In
`contrarian` mode: long when VVIX is above threshold (buy cheap vol uncertainty).
The paper documents Sharpe 2.0–3.2 for variance assets; equity adaptation is indirect.

## Research Analysis Framework

All research-to-strategy translation in the autoresearch loop follows a mandatory
5-step framework defined in `RESEARCH_ANALYSIS_FRAMEWORK` (constant in
`src/bin/autoresearch_loop.rs`). Each step is implemented by a dedicated function:

| Step | Function | Purpose |
|------|----------|---------|
| 1. Mental Model Synthesis | `research_basis()` | Core thesis, theoretical mechanism, paper citation |
| 2. Critical Evaluation | `critical_evaluation()` | Overfitting, lookahead bias, regime-dependency, transaction costs |
| 3. Trading Strategy | `rule_description()` | Precise entry/exit signal, instruments, parameters |
| 4. Asset Universe & Inputs/Outputs | `investment_case()` | Data inputs, market inefficiency, asset justification |
| 5. Backtest Architecture | `backtest_architecture()` | zonk pipeline: ingestion, features, signals, PnL simulation |

The interactive HTML report renders all 5 steps in a numbered "Research Analysis
Framework" section for each strategy candidate, plus a Performance Summary.

When adding a new rule, all 5 functions must be implemented for it.

## Universe Modes

All breadth strategies resolve universe membership locally:

| Universe | Source | Description |
|----------|--------|-------------|
| `ndx100` | `presets/ndx100.json` | NASDAQ-100 constituents (101 tickers) |
| `sp500` | `presets/sp500.json` | S&P 500 constituents |
| `r2k` | `presets/r2k.json` | Russell 2000 constituents |
| `all-stocks` | warehouse bronze dir scan | All symbols in the warehouse |
| `--preset <path>` | custom JSON file | Any custom ticker list |
| `--tickers AAPL,MSFT` | CLI argument | Explicit ticker list |

Preset JSON format: `{ "name": "ndx100", "tickers": ["AAPL", "MSFT", ...] }`

## Output Formats

All strategies support three output formats via the global `--output` flag:

| Format | Flag | Description |
|--------|------|-------------|
| Text | `--output text` | Default. Human-readable tables with progress messages. |
| JSON | `--output json` | Structured JSON object. No progress text. |
| Markdown | `--output md` | Clean markdown with headers and tables. No progress text. |

```bash
zonk --output json run overnight-drift --no-vix-filter
zonk --output md run breadth-washout --universe ndx100
zonk run intraday-drift --ticker SPY --output json
```

The flag is global and can appear before or after the subcommand.

## Install & Update

Build and install to `~/.cargo/bin` (must be in `$PATH`):

```bash
cargo build --release
cp target/release/zonk ~/.cargo/bin/zonk
```

After any code changes, rebuild and reinstall:

```bash
cargo build --release && cp target/release/zonk ~/.cargo/bin/zonk
```

## Testing

```bash
# Unit tests (226 tests: 167 lib + 59 bin, < 0.1s, no external dependencies)
cargo test

# CLI integration tests (106 tests, requires ~/market-warehouse)
./tests/cli_integration.sh
```

### Test Rules

1. 226 unit tests covering all modules (mock all I/O, use `tempfile`)
2. 106 CLI integration tests covering every command, flag combination, output format (text/json/md), and error case
3. Tests run with `set -euo pipefail` — any unexpected failure stops the suite
4. Edge cases tested: future dates, 0 sessions, missing tickers, invalid modes, invalid output formats

## Key Dependencies

- `polars` — DataFrame & parquet I/O
- `nalgebra` — Linear algebra (ADF test OLS)
- `clap` — CLI argument parsing (derive), global `--output` flag
- `serde` + `serde_json` — JSON serialization for `--output json`
- `reqwest` — HTTP client (CBOE VIX download only; no price data fetching)
- `chrono` — Date/time operations

## Performance

All price data reads from local parquet. Typical benchmarks on Apple Silicon:

| Workload | Time |
|----------|------|
| 1-year backtest (252 sessions, 101 tickers) | ~0.16s |
| 10-year backtest (2,516 sessions, 101 tickers) | ~0.32s |
| 10-year dual-MA (2,516 sessions, 101 tickers) | ~0.55s |

## Brand & Visual Design (Mandatory — All Artifacts)

**Every visual artifact** produced in this project — HTML reports, dashboards, websites, landing pages, interactive tools, data visualizations, charts, email templates, or any other browser-rendered output — **must** use the zonk design system derived from radial.org. This is non-negotiable and applies to all code paths, not just autoresearch reports.

### Design System Location

```
design/
├── tokens.css              # CSS custom properties (design tokens)
├── brand-guidelines.html   # Visual reference — open in browser to preview
└── report-template.html    # Drop-in template for autoresearch HTML reports
```

### Rules for All Visual Output

1. **Use `DESIGN.md` as the authority** for any HTML report or browser-rendered artifact.
2. **Treat the Figma `Reports` file as the target format** when it is accessible: `https://www.figma.com/design/0TCEsZxLVO6x5pJkOSOwCl/Reports?node-id=0-1&t=K0IBsu4WJyJ8qmjn-1`
3. **Use `design/report-template.html` as the implementation base** for generated reports, but keep it aligned with `DESIGN.md` rather than treating it as a fixed visual source of truth.
4. **Creative direction**: "The Digital Curator" — editorial prestige, intentional asymmetry, whitespace as authority, dense data presented as an asset.
5. **Typography**: prestige serif for display, functional sans for body, calculated sans for data/labels, consistent with `DESIGN.md`.
6. **Surface logic**: use tonal layering, ghost borders only when needed, and avoid boxed-in UI sectioning.
7. **Shape language**: default to square-cornered structure; do not use soft rounded consumer-app UI.
8. **Accent discipline**: reserve lime/high-energy accents for critical insight or action states only.
9. **Load DM Mono** from Google Fonts: `<link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@300;400;500&display=swap" rel="stylesheet" />`
10. **Scope**: These rules apply to ALL visual artifacts — not just autoresearch reports. Any HTML, dashboard, chart, website, or visual output in this project must follow this design system.

### Quick Reference: Color Palette

| Token | Hex | Use |
|-------|-----|-----|
| `--zonk-teal` | #3e5b63 | Primary brand, hero bg, footer |
| `--zonk-teal-deep` | #2e454c | Darker teal variant |
| `--zonk-lime` | #c6e758 | Accent, positive, CTA arrows |
| `--zonk-sky` | #5fc4e3 | Info, highlights, links |
| `--zonk-slate` | #4a5760 | Muted text, secondary |
| `--zonk-sage` | #c7cdc8 | Borders, neutral surface |
| `--zonk-bg` | #ffffff | Page background |
| `--zonk-bg-alt` | #f5f5f5 | Alt surface, table headers |
| `--zonk-text` | #1e1e1e | Primary text |
| `--zonk-positive-text` | #3a5200 | Gains, positive CAGR |
| `--zonk-negative-text` | #7a1a1a | Losses, drawdown |
| `--zonk-warning-text` | #7a4a00 | Warnings, mixed signals |

## How to Add a New Strategy

1. Create `src/strategies/my_strategy.rs` with a `run(args, fmt)` function
2. Define `MyStrategyArgs` using clap derive
3. Load prices with `crate::data::readers::load_price_panel()` — never fetch from external APIs
4. Load universe membership from preset JSON or CLI tickers — never call external membership APIs
5. Add the strategy to `StrategyCommand` enum in `src/cli.rs`
6. Wire it up in `src/main.rs` match arm
7. Write tests in the `#[cfg(test)] mod tests` block
8. Run `cargo test`
