#!/usr/bin/env python3
"""
Run parameter-grid backtests for mean_reversion_filter and vvix_regime rules,
then generate an interactive HTML report using the zonk design system.
"""

import json
import subprocess
import sys
import os
import html
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime

ZONK_BIN = os.path.join(os.path.dirname(__file__), "..", "target", "release", "zonk")
REPORT_PATH = os.path.join(os.path.dirname(__file__), "..", "reports", "paper-research-report.html")

ASSETS = ["SPY", "QQQ", "TQQQ", "IWM", "SPXL"]

TRAIN_START = "2020-01-01"
TRAIN_END = "2024-12-31"
TEST_START = "2025-01-01"
TEST_END = "2026-03-29"

# --- Grid definitions ---

MR_SLOW_WINDOWS = [25, 50, 70, 90, 120, 160]
MR_ENTRY_THRESHOLDS = [0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.05]

VVIX_WINDOWS = [21, 42, 63, 126, 252]
VVIX_THRESHOLDS = [0.50, 0.60, 0.70, 0.75, 0.80, 0.90]
VVIX_MODES = ["risk_off", "contrarian"]


def run_backtest(rule, asset, params, start_date, end_date):
    """Run a single zonk backtest, return parsed JSON or None on failure."""
    cmd = [
        ZONK_BIN, "--output", "json", "run", "paper-research",
        "--rule", rule,
        "--asset", asset,
        "--start-date", start_date,
        "--end-date", end_date,
    ]
    for k, v in params.items():
        cmd.extend([f"--{k}", str(v)])

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        if result.returncode != 0:
            return None
        data = json.loads(result.stdout)
        return data
    except (subprocess.TimeoutExpired, json.JSONDecodeError, Exception):
        return None


def extract_metrics(data):
    """Extract strategy metrics from JSON output (results[1] is the strategy)."""
    if not data or "results" not in data or len(data["results"]) < 2:
        return None
    r = data["results"][1]
    return {
        "cagr": r.get("cagr", 0),
        "sharpe": r.get("sharpe", 0),
        "max_drawdown": r.get("max_drawdown", 0),
        "final_equity": r.get("final_equity", 0),
        "period_start": data.get("period_start", ""),
        "period_end": data.get("period_end", ""),
    }


def build_mr_grid():
    """Build parameter grid for mean_reversion_filter."""
    grid = []
    for asset in ASSETS:
        for sw in MR_SLOW_WINDOWS:
            for thresh in MR_ENTRY_THRESHOLDS:
                grid.append({
                    "rule": "mean_reversion_filter",
                    "asset": asset,
                    "params": {
                        "slow-window": sw,
                        "mr-entry-threshold": thresh,
                    },
                    "display_params": f"sma={sw}, thr={thresh}",
                })
    return grid


def build_vvix_grid():
    """Build parameter grid for vvix_regime."""
    grid = []
    for asset in ASSETS:
        for vw in VVIX_WINDOWS:
            for vt in VVIX_THRESHOLDS:
                for vm in VVIX_MODES:
                    grid.append({
                        "rule": "vvix_regime",
                        "asset": asset,
                        "params": {
                            "vvix-window": vw,
                            "vvix-threshold": vt,
                            "vvix-mode": vm,
                        },
                        "display_params": f"win={vw}, thr={vt}, mode={vm}",
                    })
    return grid


def run_all_backtests(grid, label):
    """Run train + test backtests for all grid entries in parallel."""
    results = []
    total = len(grid)

    def run_one(entry):
        train_data = run_backtest(entry["rule"], entry["asset"], entry["params"], TRAIN_START, TRAIN_END)
        test_data = run_backtest(entry["rule"], entry["asset"], entry["params"], TEST_START, TEST_END)
        train_metrics = extract_metrics(train_data)
        test_metrics = extract_metrics(test_data)
        return {
            **entry,
            "train": train_metrics,
            "test": test_metrics,
        }

    print(f"  Running {total} backtests for {label}...", flush=True)
    completed = 0
    with ThreadPoolExecutor(max_workers=12) as executor:
        futures = {executor.submit(run_one, e): e for e in grid}
        for future in as_completed(futures):
            completed += 1
            if completed % 20 == 0 or completed == total:
                print(f"    {completed}/{total} complete", flush=True)
            result = future.result()
            if result["train"] and result["test"]:
                results.append(result)

    print(f"  {len(results)}/{total} successful for {label}")
    return results


def score_candidate(c):
    """Score = 0.6 * test_sharpe + 0.4 * train_sharpe."""
    return 0.6 * c["test"]["sharpe"] + 0.4 * c["train"]["sharpe"]


def run_audit(entry):
    """Re-run test window with --include-audit to get equity trace."""
    params = dict(entry["params"])
    cmd = [
        ZONK_BIN, "--output", "json", "run", "paper-research",
        "--rule", entry["rule"],
        "--asset", entry["asset"],
        "--start-date", TEST_START,
        "--end-date", TEST_END,
        "--include-audit",
    ]
    for k, v in params.items():
        cmd.extend([f"--{k}", str(v)])

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        if result.returncode != 0:
            return None
        return json.loads(result.stdout)
    except Exception:
        return None


def format_pct(v, decimals=2):
    """Format a decimal as percentage string."""
    if v is None:
        return "N/A"
    return f"{v * 100:.{decimals}f}%"


def format_number(v, decimals=0):
    """Format a number with comma separators."""
    if v is None:
        return "N/A"
    if decimals == 0:
        return f"{v:,.0f}"
    return f"{v:,.{decimals}f}"


def build_heatmap_data(results, rule):
    """Build asset x param heatmap data for test sharpe."""
    if rule == "mean_reversion_filter":
        param_key = "mr-entry-threshold"
        param_values = MR_ENTRY_THRESHOLDS
        secondary_key = "slow-window"
        secondary_values = MR_SLOW_WINDOWS
    else:
        param_key = "vvix-threshold"
        param_values = VVIX_THRESHOLDS
        secondary_key = "vvix-window"
        secondary_values = VVIX_WINDOWS

    filtered = [r for r in results if r["rule"] == rule]

    # Build: asset -> param_val -> best sharpe across secondary params
    heatmap = {}
    for asset in ASSETS:
        heatmap[asset] = {}
        for pv in param_values:
            candidates = [
                r for r in filtered
                if r["asset"] == asset and r["params"][param_key] == pv
            ]
            if candidates:
                best = max(candidates, key=lambda c: c["test"]["sharpe"])
                heatmap[asset][pv] = best["test"]["sharpe"]
            else:
                heatmap[asset][pv] = None

    return heatmap, param_values


def color_for_sharpe(v):
    """Return CSS color for a sharpe value."""
    if v is None:
        return "var(--on-surface-muted)"
    if v >= 1.0:
        return "#3d5a00"  # strong positive
    if v >= 0.5:
        return "#5a7a00"
    if v >= 0.0:
        return "var(--on-surface)"
    return "#6b2f2f"  # negative


def generate_html(all_results, mr_results, vvix_results, top20, top10_audits):
    """Generate the full HTML report."""

    # Load paper summaries
    paper1_path = os.path.join(os.path.dirname(__file__), "..", ".firecrawl", "ssrn-6225198-pdf.md")
    paper2_path = os.path.join(os.path.dirname(__file__), "..", ".firecrawl", "ssrn-6212458-analysis.md")

    paper1_summary = ""
    paper2_summary = ""
    try:
        with open(paper1_path) as f:
            lines = f.readlines()
            # Get title and abstract
            paper1_summary = "".join(lines[:9]).strip()
    except Exception:
        paper1_summary = "Xu et al. (2026) - Advanced Signal Filtering for Mean Reversion Trading. SSRN 6225198."

    try:
        with open(paper2_path) as f:
            lines = f.readlines()
            paper2_summary = "".join(lines[:18]).strip()
    except Exception:
        paper2_summary = "Bevilacqua & Hizmeri (2026) - Early Birds Get the Vol: Morning Volatility Uncertainty and Variance Risk Premium. SSRN 6212458."

    # Top 10 per rule
    mr_top10 = sorted([r for r in all_results if r["rule"] == "mean_reversion_filter"],
                       key=score_candidate, reverse=True)[:10]
    vvix_top10 = sorted([r for r in all_results if r["rule"] == "vvix_regime"],
                         key=score_candidate, reverse=True)[:10]

    # Heatmaps
    mr_heatmap, mr_param_values = build_heatmap_data(all_results, "mean_reversion_filter")
    vvix_heatmap, vvix_param_values = build_heatmap_data(all_results, "vvix_regime")

    # Vvix mode breakdown
    vvix_mode_best = {}
    for mode in VVIX_MODES:
        mode_results = [r for r in all_results if r["rule"] == "vvix_regime" and r["params"]["vvix-mode"] == mode]
        if mode_results:
            best = max(mode_results, key=score_candidate)
            vvix_mode_best[mode] = best

    # Build equity chart data for top 10 audits
    equity_charts_js = "const equityData = {};\n"
    for i, audit in enumerate(top10_audits):
        if audit and "audit" in audit and "equity_trace" in audit["audit"]:
            trace = audit["audit"]["equity_trace"]
            dates = [p["date"] for p in trace]
            equities = [p["equity"] for p in trace]
            equity_charts_js += f"equityData[{i}] = {{dates: {json.dumps(dates)}, equities: {json.dumps(equities)}}};\n"

    now = datetime.now().strftime("%Y-%m-%d %H:%M")
    mr_total = len([r for r in all_results if r["rule"] == "mean_reversion_filter"])
    vvix_total = len([r for r in all_results if r["rule"] == "vvix_regime"])

    report = f'''<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Paper-Derived Strategy Research Report</title>
  <link rel="preconnect" href="https://fonts.googleapis.com" />
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=Newsreader:opsz,wght@6..72,400;6..72,500;6..72,600&family=Space+Grotesk:wght@400;500;700&family=DM+Mono:wght@300;400;500&display=swap" rel="stylesheet" />
  <style>
    :root {{
      --surface: #f3fbf8;
      --surface-container-low: #edf5f2;
      --surface-container: #e8f0ed;
      --surface-container-high: #dce4e1;
      --surface-container-lowest: #ffffff;
      --on-surface: #151d1c;
      --on-surface-muted: #4e635c;
      --on-surface-soft: #6f7978;
      --primary: #003434;
      --primary-container: #004d4d;
      --secondary: #4e635c;
      --secondary-container: #cee5dc;
      --outline: #6f7978;
      --outline-variant: #bfc8c8;
      --accent-lime: #c3f400;
      --accent-lime-deep: #abd600;
      --ink-dark: #161e00;
      --shadow: 0 24px 48px -12px rgba(21, 29, 28, 0.04);
      --glass: rgba(243, 251, 248, 0.8);
      --ghost-line: rgba(191, 200, 200, 0.2);
      --ghost-line-strong: rgba(191, 200, 200, 0.3);
      --transition: 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
      --positive: #3d5a00;
      --negative: #6b2f2f;
    }}

    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    html {{ scroll-behavior: smooth; }}

    body {{
      min-height: 100vh;
      background: linear-gradient(180deg, var(--surface) 0%, var(--surface-container-low) 100%);
      color: var(--on-surface);
      font-family: "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      -webkit-font-smoothing: antialiased;
      text-rendering: optimizeLegibility;
      line-height: 1.5;
    }}

    body::before {{
      content: "";
      position: fixed;
      inset: 0;
      pointer-events: none;
      z-index: -1;
      background:
        radial-gradient(circle at top left, rgba(206, 229, 220, 0.42) 0%, transparent 24%),
        radial-gradient(circle at top right, rgba(195, 244, 0, 0.12) 0%, transparent 18%);
    }}

    .page {{ max-width: 1540px; margin: 0 auto; padding: 0 clamp(20px, calc(12px + 1.8vw), 40px); }}

    /* --- Hero --- */
    .hero {{
      padding: 80px 0 48px;
      border-bottom: 1px solid var(--ghost-line);
    }}
    .hero-eyebrow {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 500;
      letter-spacing: 0.05rem;
      text-transform: uppercase;
      color: var(--on-surface-muted);
      margin-bottom: 12px;
    }}
    .hero h1 {{
      font-family: "Newsreader", Georgia, serif;
      font-size: clamp(2rem, 4vw, 2.625rem);
      font-weight: 400;
      line-height: 1.1;
      letter-spacing: -0.02em;
      color: var(--primary);
      margin-bottom: 16px;
    }}
    .hero-meta {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.8125rem;
      color: var(--on-surface-muted);
    }}
    .hero-meta span {{ margin-right: 24px; }}

    /* --- Stats bar --- */
    .stats-bar {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
      gap: 1px;
      background: var(--ghost-line);
      margin: 32px 0;
    }}
    .stat {{
      background: var(--surface-container-lowest);
      padding: 20px 24px;
    }}
    .stat-label {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 500;
      letter-spacing: 0.05rem;
      text-transform: uppercase;
      color: var(--on-surface-muted);
      margin-bottom: 4px;
    }}
    .stat-value {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 1.75rem;
      font-weight: 700;
      color: var(--primary);
      line-height: 1.0;
    }}
    .stat-value.positive {{ color: var(--positive); }}
    .stat-value.negative {{ color: var(--negative); }}

    /* --- Section --- */
    .section {{
      padding: 48px 0 32px;
      border-bottom: 1px solid var(--ghost-line);
    }}
    .section:last-child {{ border-bottom: none; }}
    .section-eyebrow {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 500;
      letter-spacing: 0.05rem;
      text-transform: uppercase;
      color: var(--on-surface-muted);
      margin-bottom: 8px;
    }}
    .section h2 {{
      font-family: "Newsreader", Georgia, serif;
      font-size: 1.75rem;
      font-weight: 400;
      color: var(--primary);
      margin-bottom: 24px;
      line-height: 1.2;
    }}
    .section h3 {{
      font-family: "Newsreader", Georgia, serif;
      font-size: 1.25rem;
      font-weight: 500;
      color: var(--primary);
      margin-bottom: 16px;
      margin-top: 32px;
    }}

    /* --- Paper cards --- */
    .paper-card {{
      background: var(--surface-container-lowest);
      padding: 28px 32px;
      margin-bottom: 16px;
      box-shadow: var(--shadow);
      border-left: 3px solid var(--primary);
    }}
    .paper-card h4 {{
      font-family: "Newsreader", Georgia, serif;
      font-size: 1.1rem;
      font-weight: 500;
      color: var(--primary);
      margin-bottom: 8px;
    }}
    .paper-card .authors {{
      font-size: 0.8125rem;
      color: var(--on-surface-muted);
      margin-bottom: 12px;
    }}
    .paper-card p {{
      font-size: 0.875rem;
      line-height: 1.65;
      color: var(--on-surface);
    }}
    .paper-card .tag {{
      display: inline-block;
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 500;
      letter-spacing: 0.03em;
      background: var(--secondary-container);
      color: var(--primary);
      padding: 2px 10px;
      margin-right: 6px;
      margin-top: 10px;
    }}

    /* --- Tables --- */
    .table-wrap {{
      overflow-x: auto;
      margin: 16px 0;
    }}
    table {{
      width: 100%;
      border-collapse: collapse;
      font-size: 0.8125rem;
    }}
    thead th {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 600;
      letter-spacing: 0.04em;
      text-transform: uppercase;
      color: var(--on-surface-muted);
      background: var(--surface-container-low);
      padding: 10px 14px;
      text-align: left;
      white-space: nowrap;
      border-bottom: 1px solid var(--outline-variant);
    }}
    thead th.num {{ text-align: right; }}
    tbody td {{
      font-family: "Space Grotesk", sans-serif;
      padding: 10px 14px;
      border-bottom: 1px solid var(--ghost-line);
      vertical-align: middle;
    }}
    tbody td.num {{
      text-align: right;
      font-variant-numeric: tabular-nums;
    }}
    tbody tr:hover {{ background: var(--surface-container-low); }}
    tbody tr.highlight {{
      background: rgba(195, 244, 0, 0.06);
    }}
    .positive-val {{ color: var(--positive); font-weight: 600; }}
    .negative-val {{ color: var(--negative); font-weight: 600; }}
    .rank-badge {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 24px;
      height: 24px;
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      font-weight: 700;
      background: var(--primary);
      color: var(--accent-lime);
    }}
    .rank-badge.top3 {{
      background: var(--accent-lime);
      color: var(--ink-dark);
    }}

    /* --- Heatmap --- */
    .heatmap {{
      margin: 16px 0;
    }}
    .heatmap table {{
      font-size: 0.75rem;
    }}
    .heatmap td {{
      text-align: center;
      padding: 8px 12px;
      font-family: "DM Mono", monospace;
      font-weight: 400;
      min-width: 72px;
    }}
    .heatmap td.asset-label {{
      font-family: "Space Grotesk", sans-serif;
      font-weight: 600;
      text-align: left;
      color: var(--primary);
    }}
    .heat-strong {{ background: rgba(61, 90, 0, 0.15); color: var(--positive); font-weight: 600; }}
    .heat-good {{ background: rgba(61, 90, 0, 0.08); color: var(--positive); }}
    .heat-neutral {{ background: transparent; }}
    .heat-weak {{ background: rgba(107, 47, 47, 0.06); color: var(--on-surface-muted); }}
    .heat-bad {{ background: rgba(107, 47, 47, 0.12); color: var(--negative); font-weight: 600; }}

    /* --- Charts --- */
    .chart-container {{
      background: var(--surface-container-lowest);
      padding: 24px;
      margin: 16px 0;
      box-shadow: var(--shadow);
    }}
    .chart-title {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.8125rem;
      font-weight: 600;
      color: var(--primary);
      margin-bottom: 16px;
    }}
    canvas {{ width: 100% !important; height: 200px !important; }}

    /* --- Methodology --- */
    .methodology {{
      background: var(--surface-container-lowest);
      padding: 28px 32px;
      margin: 16px 0;
      box-shadow: var(--shadow);
    }}
    .methodology h4 {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.875rem;
      font-weight: 600;
      color: var(--primary);
      margin-bottom: 12px;
    }}
    .methodology ul {{
      list-style: none;
      padding: 0;
    }}
    .methodology li {{
      font-size: 0.8125rem;
      line-height: 1.7;
      color: var(--on-surface-muted);
      padding-left: 16px;
      position: relative;
    }}
    .methodology li::before {{
      content: "\\2014";
      position: absolute;
      left: 0;
      color: var(--outline-variant);
    }}

    /* --- Footer --- */
    .footer {{
      padding: 48px 0 32px;
      text-align: center;
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.6875rem;
      color: var(--on-surface-soft);
      letter-spacing: 0.03em;
    }}

    /* --- Collapsible --- */
    details {{ margin: 12px 0; }}
    summary {{
      cursor: pointer;
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.8125rem;
      font-weight: 600;
      color: var(--primary);
      padding: 8px 0;
    }}
    summary:hover {{ color: var(--primary-container); }}

    /* --- Tabs --- */
    .tab-bar {{
      display: flex;
      gap: 0;
      border-bottom: 1px solid var(--outline-variant);
      margin-bottom: 24px;
    }}
    .tab-btn {{
      font-family: "Space Grotesk", sans-serif;
      font-size: 0.8125rem;
      font-weight: 500;
      color: var(--on-surface-muted);
      background: none;
      border: none;
      padding: 10px 20px;
      border-bottom: 2px solid transparent;
      transition: var(--transition);
    }}
    .tab-btn:hover {{ color: var(--primary); }}
    .tab-btn.active {{
      color: var(--primary);
      border-bottom-color: var(--accent-lime);
      font-weight: 600;
    }}
    .tab-panel {{ display: none; }}
    .tab-panel.active {{ display: block; }}
  </style>
</head>
<body>
<div class="page">

  <!-- Hero -->
  <div class="hero">
    <div class="hero-eyebrow">zonk Research</div>
    <h1>Paper-Derived Strategy Research Report</h1>
    <div class="hero-meta">
      <span>Generated {now}</span>
      <span>Train: {TRAIN_START} to {TRAIN_END}</span>
      <span>Test: {TEST_START} to {TEST_END}</span>
    </div>
  </div>

  <!-- Stats bar -->
  <div class="stats-bar">
    <div class="stat">
      <div class="stat-label">Total Backtests</div>
      <div class="stat-value">{format_number(len(all_results))}</div>
    </div>
    <div class="stat">
      <div class="stat-label">Mean Reversion</div>
      <div class="stat-value">{format_number(mr_total)}</div>
    </div>
    <div class="stat">
      <div class="stat-label">VVIX Regime</div>
      <div class="stat-value">{format_number(vvix_total)}</div>
    </div>
    <div class="stat">
      <div class="stat-label">Best Test Sharpe</div>
      <div class="stat-value positive">{top20[0]["test"]["sharpe"]:.2f}</div>
    </div>
    <div class="stat">
      <div class="stat-label">Best Test CAGR</div>
      <div class="stat-value positive">{format_pct(top20[0]["test"]["cagr"])}</div>
    </div>
  </div>

  <!-- Paper Summaries -->
  <div class="section">
    <div class="section-eyebrow">Source Literature</div>
    <h2>Academic Papers</h2>

    <div class="paper-card">
      <h4>Advanced Signal Filtering for Mean Reversion Trading</h4>
      <div class="authors">Xu, Firoozye, Koukorinis, Treleaven, Zhu (UCL, 2026) &mdash; SSRN 6225198</div>
      <p>Develops a regime-aware mean-reversion framework using adaptive signal filters (LAFO) to determine latent fair price. The spot-filter spread yields buy/sell signals when price deviates beyond a threshold from the SMA-estimated fair value. Neural network filters (RNN, CNN, S4) approximate solutions within this framework. Our zonk implementation uses the simplified SMA-based spread: &delta; = (close &minus; SMA) / SMA; go long when &delta; &lt; &minus;threshold.</p>
      <span class="tag">Mean Reversion</span>
      <span class="tag">Signal Filtering</span>
      <span class="tag">Regime Switching</span>
    </div>

    <div class="paper-card">
      <h4>Early Birds Get the Vol: Morning Volatility Uncertainty and Variance Risk Premium</h4>
      <div class="authors">Bevilacqua, Hizmeri (U. Liverpool, 2026) &mdash; SSRN 6212458</div>
      <p>Morning VVIX (10:00 EST) strongly predicts next-day variance asset returns due to limited attention and slow-moving beliefs. The signal achieves Sharpe ratios of 2.09&ndash;3.24 on VIX futures (long-only, 75th percentile threshold). Our zonk adaptation uses daily VVIX close as a proxy, computing a rolling percentile to generate risk_off (long when VVIX low, calm regime) or contrarian (long when VVIX high, elevated uncertainty) signals on equity assets.</p>
      <span class="tag">Volatility of Volatility</span>
      <span class="tag">Variance Risk Premium</span>
      <span class="tag">Limited Attention</span>
    </div>
  </div>

  <!-- Combined Leaderboard -->
  <div class="section">
    <div class="section-eyebrow">Combined Rankings</div>
    <h2>Top 20 Strategy Leaderboard</h2>
    <p style="font-size: 0.8125rem; color: var(--on-surface-muted); margin-bottom: 16px;">
      Score = 0.6 &times; Test Sharpe + 0.4 &times; Train Sharpe. Ranked by blended score favoring out-of-sample performance.
    </p>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Rank</th>
            <th>Rule</th>
            <th>Asset</th>
            <th>Parameters</th>
            <th class="num">Train Sharpe</th>
            <th class="num">Test Sharpe</th>
            <th class="num">Test CAGR</th>
            <th class="num">Test Drawdown</th>
            <th class="num">Score</th>
          </tr>
        </thead>
        <tbody>
'''

    for i, c in enumerate(top20):
        rank = i + 1
        badge_class = "rank-badge top3" if rank <= 3 else "rank-badge"
        highlight = ' class="highlight"' if rank <= 3 else ""
        train_sharpe = c["train"]["sharpe"]
        test_sharpe = c["test"]["sharpe"]
        test_cagr = c["test"]["cagr"]
        test_dd = c["test"]["max_drawdown"]
        sc = score_candidate(c)

        sharpe_class = "positive-val" if test_sharpe > 0 else "negative-val"
        cagr_class = "positive-val" if test_cagr > 0 else "negative-val"
        dd_class = "negative-val" if test_dd > 0.15 else ""

        report += f'''          <tr{highlight}>
            <td><span class="{badge_class}">{rank}</span></td>
            <td>{html.escape(c["rule"])}</td>
            <td style="font-weight:600">{html.escape(c["asset"])}</td>
            <td style="font-family:'DM Mono',monospace;font-size:0.75rem">{html.escape(c["display_params"])}</td>
            <td class="num">{train_sharpe:.3f}</td>
            <td class="num {sharpe_class}">{test_sharpe:.3f}</td>
            <td class="num {cagr_class}">{format_pct(test_cagr)}</td>
            <td class="num {dd_class}">{format_pct(test_dd)}</td>
            <td class="num" style="font-weight:700">{sc:.3f}</td>
          </tr>
'''

    report += '''        </tbody>
      </table>
    </div>
  </div>

'''

    # --- Per-rule sections with tabs ---
    report += '''  <!-- Per-Rule Analysis -->
  <div class="section">
    <div class="section-eyebrow">Detailed Analysis</div>
    <h2>Per-Rule Breakdown</h2>

    <div class="tab-bar">
      <button class="tab-btn active" onclick="switchTab(\'mr\')">mean_reversion_filter</button>
      <button class="tab-btn" onclick="switchTab(\'vvix\')">vvix_regime</button>
    </div>

    <!-- Mean Reversion Tab -->
    <div class="tab-panel active" id="tab-mr">
      <h3>Parameter Sensitivity: Test Sharpe by Asset and Entry Threshold</h3>
      <p style="font-size:0.8125rem;color:var(--on-surface-muted);margin-bottom:12px">
        Best Sharpe across all SMA window sizes for each asset/threshold combination.
      </p>
      <div class="heatmap">
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Asset</th>
'''

    for pv in mr_param_values:
        report += f'                <th class="num">thr={pv}</th>\n'
    report += '              </tr>\n            </thead>\n            <tbody>\n'

    for asset in ASSETS:
        report += f'              <tr>\n                <td class="asset-label">{asset}</td>\n'
        for pv in mr_param_values:
            v = mr_heatmap[asset].get(pv)
            if v is not None:
                heat_class = "heat-strong" if v >= 1.0 else "heat-good" if v >= 0.5 else "heat-neutral" if v >= 0 else "heat-weak" if v >= -0.5 else "heat-bad"
                report += f'                <td class="num {heat_class}">{v:.2f}</td>\n'
            else:
                report += '                <td class="num">-</td>\n'
        report += '              </tr>\n'

    report += '''            </tbody>
          </table>
        </div>
      </div>

      <h3>Top 10 mean_reversion_filter Candidates</h3>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Rank</th>
              <th>Asset</th>
              <th>SMA Window</th>
              <th>Entry Threshold</th>
              <th class="num">Train Sharpe</th>
              <th class="num">Train CAGR</th>
              <th class="num">Test Sharpe</th>
              <th class="num">Test CAGR</th>
              <th class="num">Test Drawdown</th>
              <th class="num">Test Final Equity</th>
            </tr>
          </thead>
          <tbody>
'''

    for i, c in enumerate(mr_top10):
        rank = i + 1
        report += f'''            <tr>
              <td><span class="rank-badge{' top3' if rank <= 3 else ''}">{rank}</span></td>
              <td style="font-weight:600">{c["asset"]}</td>
              <td class="num">{c["params"]["slow-window"]}</td>
              <td class="num">{c["params"]["mr-entry-threshold"]}</td>
              <td class="num">{c["train"]["sharpe"]:.3f}</td>
              <td class="num">{format_pct(c["train"]["cagr"])}</td>
              <td class="num {("positive-val" if c["test"]["sharpe"] > 0 else "negative-val")}">{c["test"]["sharpe"]:.3f}</td>
              <td class="num {("positive-val" if c["test"]["cagr"] > 0 else "negative-val")}">{format_pct(c["test"]["cagr"])}</td>
              <td class="num">{format_pct(c["test"]["max_drawdown"])}</td>
              <td class="num">${format_number(c["test"]["final_equity"])}</td>
            </tr>
'''

    report += '''          </tbody>
        </table>
      </div>
    </div>

    <!-- VVIX Regime Tab -->
    <div class="tab-panel" id="tab-vvix">
      <h3>Parameter Sensitivity: Test Sharpe by Asset and VVIX Threshold</h3>
      <p style="font-size:0.8125rem;color:var(--on-surface-muted);margin-bottom:12px">
        Best Sharpe across all VVIX windows and modes for each asset/threshold combination.
      </p>
      <div class="heatmap">
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Asset</th>
'''

    for pv in vvix_param_values:
        report += f'                <th class="num">thr={pv}</th>\n'
    report += '              </tr>\n            </thead>\n            <tbody>\n'

    for asset in ASSETS:
        report += f'              <tr>\n                <td class="asset-label">{asset}</td>\n'
        for pv in vvix_param_values:
            v = vvix_heatmap[asset].get(pv)
            if v is not None:
                heat_class = "heat-strong" if v >= 1.0 else "heat-good" if v >= 0.5 else "heat-neutral" if v >= 0 else "heat-weak" if v >= -0.5 else "heat-bad"
                report += f'                <td class="num {heat_class}">{v:.2f}</td>\n'
            else:
                report += '                <td class="num">-</td>\n'
        report += '              </tr>\n'

    # VVIX mode comparison
    report += '''            </tbody>
          </table>
        </div>
      </div>

      <h3>Mode Comparison: risk_off vs contrarian</h3>
      <p style="font-size:0.8125rem;color:var(--on-surface-muted);margin-bottom:12px">
        Best candidate from each VVIX mode.
      </p>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Mode</th>
              <th>Best Asset</th>
              <th>Parameters</th>
              <th class="num">Train Sharpe</th>
              <th class="num">Test Sharpe</th>
              <th class="num">Test CAGR</th>
              <th class="num">Score</th>
            </tr>
          </thead>
          <tbody>
'''

    for mode in VVIX_MODES:
        if mode in vvix_mode_best:
            c = vvix_mode_best[mode]
            sc = score_candidate(c)
            report += f'''            <tr>
              <td style="font-weight:600">{mode}</td>
              <td>{c["asset"]}</td>
              <td style="font-family:'DM Mono',monospace;font-size:0.75rem">{c["display_params"]}</td>
              <td class="num">{c["train"]["sharpe"]:.3f}</td>
              <td class="num">{c["test"]["sharpe"]:.3f}</td>
              <td class="num">{format_pct(c["test"]["cagr"])}</td>
              <td class="num" style="font-weight:700">{sc:.3f}</td>
            </tr>
'''

    report += '''          </tbody>
        </table>
      </div>

      <h3>Top 10 vvix_regime Candidates</h3>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Rank</th>
              <th>Asset</th>
              <th>VVIX Window</th>
              <th>Threshold</th>
              <th>Mode</th>
              <th class="num">Train Sharpe</th>
              <th class="num">Train CAGR</th>
              <th class="num">Test Sharpe</th>
              <th class="num">Test CAGR</th>
              <th class="num">Test Drawdown</th>
              <th class="num">Test Final Equity</th>
            </tr>
          </thead>
          <tbody>
'''

    for i, c in enumerate(vvix_top10):
        rank = i + 1
        report += f'''            <tr>
              <td><span class="rank-badge{' top3' if rank <= 3 else ''}">{rank}</span></td>
              <td style="font-weight:600">{c["asset"]}</td>
              <td class="num">{c["params"]["vvix-window"]}</td>
              <td class="num">{c["params"]["vvix-threshold"]}</td>
              <td>{c["params"]["vvix-mode"]}</td>
              <td class="num">{c["train"]["sharpe"]:.3f}</td>
              <td class="num">{format_pct(c["train"]["cagr"])}</td>
              <td class="num {("positive-val" if c["test"]["sharpe"] > 0 else "negative-val")}">{c["test"]["sharpe"]:.3f}</td>
              <td class="num {("positive-val" if c["test"]["cagr"] > 0 else "negative-val")}">{format_pct(c["test"]["cagr"])}</td>
              <td class="num">{format_pct(c["test"]["max_drawdown"])}</td>
              <td class="num">${format_number(c["test"]["final_equity"])}</td>
            </tr>
'''

    report += '''          </tbody>
        </table>
      </div>
    </div>
  </div>

'''

    # --- Equity traces for top 10 ---
    report += '''  <!-- Equity Traces -->
  <div class="section">
    <div class="section-eyebrow">Performance Visualization</div>
    <h2>Test-Window Equity Curves (Top 10)</h2>
    <p style="font-size:0.8125rem;color:var(--on-surface-muted);margin-bottom:16px">
      Equity curves from the out-of-sample test period ({test_start} to {test_end}). Starting capital: $1,000,000.
    </p>
'''.format(test_start=TEST_START, test_end=TEST_END)

    for i, c in enumerate(top20[:10]):
        rank = i + 1
        audit = top10_audits[i] if i < len(top10_audits) else None
        has_trace = audit and "audit" in audit and "equity_trace" in audit["audit"]
        report += f'''    <div class="chart-container">
      <div class="chart-title">#{rank} {html.escape(c["rule"])} | {html.escape(c["asset"])} | {html.escape(c["display_params"])}</div>
      <div style="display:flex;gap:32px;font-family:'Space Grotesk',sans-serif;font-size:0.8125rem;margin-bottom:12px;color:var(--on-surface-muted);">
        <span>Sharpe: <strong style="color:var(--primary)">{c["test"]["sharpe"]:.3f}</strong></span>
        <span>CAGR: <strong style="color:{("var(--positive)" if c["test"]["cagr"] > 0 else "var(--negative)")}">{format_pct(c["test"]["cagr"])}</strong></span>
        <span>Drawdown: <strong>{format_pct(c["test"]["max_drawdown"])}</strong></span>
      </div>
      <canvas id="chart-{i}" height="200"></canvas>
    </div>
'''

    report += '  </div>\n\n'

    # --- Methodology ---
    report += f'''  <!-- Methodology -->
  <div class="section">
    <div class="section-eyebrow">Methodology</div>
    <h2>Backtest Parameters and Notes</h2>

    <div class="methodology">
      <h4>Execution Details</h4>
      <ul>
        <li>All backtests executed via <code>zonk run paper-research --output json</code></li>
        <li>Capital: $1,000,000 per backtest</li>
        <li>Fee model: IBKR Tiered (round-trip cost deducted per trade)</li>
        <li>Train window: {TRAIN_START} to {TRAIN_END} (~5 years, ~1,260 sessions)</li>
        <li>Test window: {TEST_START} to {TEST_END} (~1.25 years, ~315 sessions)</li>
        <li>Price data: local parquet from ~/market-warehouse (split-adjusted, not dividend-adjusted)</li>
        <li>VIX/VVIX data: local parquet at asset_class=volatility/symbol=VIX</li>
      </ul>
    </div>

    <div class="methodology">
      <h4>mean_reversion_filter Grid</h4>
      <ul>
        <li>Assets: {", ".join(ASSETS)}</li>
        <li>SMA windows: {", ".join(str(x) for x in MR_SLOW_WINDOWS)}</li>
        <li>Entry thresholds: {", ".join(str(x) for x in MR_ENTRY_THRESHOLDS)}</li>
        <li>Total combinations: {len(ASSETS)} x {len(MR_SLOW_WINDOWS)} x {len(MR_ENTRY_THRESHOLDS)} = {len(ASSETS) * len(MR_SLOW_WINDOWS) * len(MR_ENTRY_THRESHOLDS)}</li>
      </ul>
    </div>

    <div class="methodology">
      <h4>vvix_regime Grid</h4>
      <ul>
        <li>Assets: {", ".join(ASSETS)}</li>
        <li>VVIX windows: {", ".join(str(x) for x in VVIX_WINDOWS)}</li>
        <li>VVIX thresholds: {", ".join(str(x) for x in VVIX_THRESHOLDS)}</li>
        <li>Modes: risk_off, contrarian</li>
        <li>Total combinations: {len(ASSETS)} x {len(VVIX_WINDOWS)} x {len(VVIX_THRESHOLDS)} x 2 = {len(ASSETS) * len(VVIX_WINDOWS) * len(VVIX_THRESHOLDS) * 2}</li>
      </ul>
    </div>

    <div class="methodology">
      <h4>Scoring</h4>
      <ul>
        <li>Score = 0.6 x Test Sharpe + 0.4 x Train Sharpe</li>
        <li>Blended score weights out-of-sample performance more heavily to penalize overfitting</li>
        <li>Top 20 overall (top 10 per rule) selected for the leaderboard</li>
        <li>Equity curves generated for the overall top 10 using --include-audit</li>
      </ul>
    </div>

    <div class="methodology">
      <h4>Caveats</h4>
      <ul>
        <li>adj_close == close in this warehouse (IB TRADES data). Buy-and-hold understates true total return by ~1.3%/yr due to missing dividends</li>
        <li>VVIX proxy uses daily VIX close data, not intraday 10:00 EST VVIX as in the original paper</li>
        <li>Mean reversion filter uses simple SMA spread, not the neural LAFO filter from the original paper</li>
        <li>Results are not forward-looking: test window may include recent market regime shifts</li>
      </ul>
    </div>
  </div>

  <!-- Footer -->
  <div class="footer">
    zonk Quantitative Research &middot; Generated {now} &middot; All data from local parquet warehouse
  </div>

</div>

<script src="https://cdn.jsdelivr.net/npm/chart.js@4/dist/chart.umd.min.js"></script>
<script>
// Tab switching
function switchTab(id) {{
  document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
  document.querySelectorAll('.tab-panel').forEach(p => p.classList.remove('active'));
  document.getElementById('tab-' + id).classList.add('active');
  event.target.classList.add('active');
}}

// Equity chart data
{equity_charts_js}

// Render equity charts
document.addEventListener('DOMContentLoaded', function() {{
  const chartColors = [
    '#003434', '#004d4d', '#3d5a00', '#7aa7a1', '#4e635c',
    '#c3f400', '#6f7978', '#abd600', '#bfc8c8', '#151d1c'
  ];

  for (let i = 0; i < 10; i++) {{
    const canvas = document.getElementById('chart-' + i);
    if (!canvas || !equityData[i]) continue;

    const data = equityData[i];
    new Chart(canvas, {{
      type: 'line',
      data: {{
        labels: data.dates,
        datasets: [{{
          label: 'Equity',
          data: data.equities,
          borderColor: chartColors[i % chartColors.length],
          borderWidth: 1.5,
          fill: false,
          pointRadius: 0,
          tension: 0.1,
        }}]
      }},
      options: {{
        responsive: true,
        maintainAspectRatio: false,
        plugins: {{
          legend: {{ display: false }},
          tooltip: {{
            callbacks: {{
              label: function(ctx) {{
                return '$' + ctx.parsed.y.toLocaleString(undefined, {{maximumFractionDigits: 0}});
              }}
            }}
          }}
        }},
        scales: {{
          x: {{
            display: true,
            ticks: {{
              maxTicksLimit: 8,
              font: {{ family: "'Space Grotesk'", size: 10 }},
              color: '#6f7978'
            }},
            grid: {{ display: false }}
          }},
          y: {{
            display: true,
            ticks: {{
              callback: function(v) {{ return '$' + (v/1000).toFixed(0) + 'k'; }},
              font: {{ family: "'Space Grotesk'", size: 10 }},
              color: '#6f7978'
            }},
            grid: {{ color: 'rgba(191, 200, 200, 0.2)' }}
          }}
        }}
      }}
    }});
  }}
}});
</script>

</body>
</html>'''

    return report


def main():
    print("=" * 60)
    print("Paper-Derived Strategy Research Report Generator")
    print("=" * 60)

    # Build grids
    mr_grid = build_mr_grid()
    vvix_grid = build_vvix_grid()

    print(f"\nGrid sizes: MR={len(mr_grid)}, VVIX={len(vvix_grid)}, Total={len(mr_grid)+len(vvix_grid)}")
    print(f"Each candidate needs train + test = {(len(mr_grid)+len(vvix_grid))*2} total backtest runs\n")

    # Run backtests
    print("[1/4] Running mean_reversion_filter backtests...")
    mr_results = run_all_backtests(mr_grid, "mean_reversion_filter")

    print("\n[2/4] Running vvix_regime backtests...")
    vvix_results = run_all_backtests(vvix_grid, "vvix_regime")

    all_results = mr_results + vvix_results
    print(f"\nTotal successful: {len(all_results)}")

    if not all_results:
        print("ERROR: No successful backtests. Aborting.")
        sys.exit(1)

    # Score and rank
    for c in all_results:
        c["score"] = score_candidate(c)

    all_results.sort(key=lambda c: c["score"], reverse=True)
    top20 = all_results[:20]

    print("\n[3/4] Running audit backtests for top 10...")
    top10_audits = []
    with ThreadPoolExecutor(max_workers=6) as executor:
        futures = [executor.submit(run_audit, c) for c in top20[:10]]
        for f in futures:
            top10_audits.append(f.result())
    audit_count = sum(1 for a in top10_audits if a and "audit" in a)
    print(f"  {audit_count}/10 audits successful")

    # Generate report
    print("\n[4/4] Generating HTML report...")
    report_html = generate_html(all_results, mr_results, vvix_results, top20, top10_audits)

    os.makedirs(os.path.dirname(REPORT_PATH), exist_ok=True)
    with open(REPORT_PATH, "w") as f:
        f.write(report_html)

    print(f"\nReport written to: {REPORT_PATH}")
    print(f"Top 3 candidates:")
    for i, c in enumerate(top20[:3]):
        print(f"  #{i+1}: {c['rule']} | {c['asset']} | {c['display_params']}")
        print(f"       Train Sharpe: {c['train']['sharpe']:.3f}, Test Sharpe: {c['test']['sharpe']:.3f}, Score: {c['score']:.3f}")


if __name__ == "__main__":
    main()
