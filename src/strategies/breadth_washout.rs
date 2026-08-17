/// Generic breadth washout forward-return strategy.
///
/// Signal: compute the share of a universe that closed above/below its SMA.
/// Trigger oversold/overbought at a configured threshold.
///
/// All price data is read from local warehouse parquet files via
/// `load_price_panel()`. Universe membership is resolved from preset JSON
/// files — no external API calls for prices or membership.
///
/// Features:
/// - Local parquet price loading (warehouse bronze layer)
/// - Preset-based universe membership (ndx100, sp500, r2k, custom)
/// - Point-in-time breadth computation
/// - Forward-return summarization conditioned on signal triggers
/// - Membership change tracking
/// - Multiple universe modes: preset, tickers, all-stocks
/// - CSV/JSON output artifacts
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::cli::OutputFormat;
use crate::config::{output_dir, presets_dir, warehouse_root};
use crate::data::discovery::discover_symbols;
use crate::data::readers::load_price_panel;
use crate::strategies::ndx100_sma_breadth::{BreadthRow, DEFAULT_FORWARD_HORIZONS};

const STRATEGY_SLUG: &str = "breadth_washout";
const NASDAQ_WEIGHTING_URL: &str = "https://indexes.nasdaqomx.com/Index/WeightingData";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0";

const DEFAULT_NDX_SNAPSHOT_DATES: &[&str] = &[
    "2025-03-11",
    "2025-05-19",
    "2025-07-25",
    "2025-07-28",
    "2025-11-07",
    "2025-11-10",
    "2025-12-22",
    "2026-01-05",
    "2026-01-16",
    "2026-01-20",
];

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BreadthWashoutConfig {
    pub end_date: String,
    pub sessions: usize,
    pub lookback: usize,
    pub signal_mode: String,
    pub signal_threshold: f64,
    pub universe_mode: String,
    pub universe_label: String,
    pub index_symbol: Option<String>,
    pub membership_time_of_day: String,
    pub membership_snapshot_dates: Vec<String>,
    pub preset_path: Option<String>,
    pub explicit_tickers: Vec<String>,
    pub bronze_dir: Option<String>,
    pub forward_assets: Vec<String>,
    pub horizons: Vec<(String, usize)>,
    pub adjusted_forward_returns: bool,
    pub max_workers: usize,
}

impl Default for BreadthWashoutConfig {
    fn default() -> Self {
        Self {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            lookback: 5,
            signal_mode: "oversold".to_string(),
            signal_threshold: 65.0,
            universe_mode: "preset".to_string(),
            universe_label: "ndx100".to_string(),
            index_symbol: None,
            membership_time_of_day: "EOD".to_string(),
            membership_snapshot_dates: Vec::new(),
            preset_path: None,
            explicit_tickers: Vec::new(),
            bronze_dir: None,
            forward_assets: vec!["SPY".to_string(), "SPXL".to_string()],
            horizons: DEFAULT_FORWARD_HORIZONS
                .iter()
                .map(|(l, s)| (l.to_string(), *s))
                .collect(),
            adjusted_forward_returns: true,
            max_workers: 12,
        }
    }
}

// ---------------------------------------------------------------------------
// Named universes
// ---------------------------------------------------------------------------

struct NamedUniverse {
    mode: &'static str,
    label: &'static str,
    index_symbol: Option<&'static str>,
    preset_name: Option<&'static str>,
}

fn named_universes() -> Vec<(&'static str, NamedUniverse)> {
    vec![
        (
            "ndx100",
            NamedUniverse {
                mode: "preset",
                label: "ndx100",
                index_symbol: None,
                preset_name: Some("ndx100"),
            },
        ),
        (
            "sp500",
            NamedUniverse {
                mode: "preset",
                label: "sp500",
                index_symbol: None,
                preset_name: Some("sp500"),
            },
        ),
        (
            "r2k",
            NamedUniverse {
                mode: "preset",
                label: "r2k",
                index_symbol: None,
                preset_name: Some("r2k"),
            },
        ),
        (
            "all-stocks",
            NamedUniverse {
                mode: "all-stocks",
                label: "all-stocks",
                index_symbol: None,
                preset_name: None,
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn slugify(value: &str) -> String {
    let lower = value.to_lowercase();
    let slugged: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut result = String::new();
    let mut prev_dash = true;
    for c in slugged.chars() {
        if c == '-' {
            if !prev_dash {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    result.trim_matches('-').to_string()
}

fn threshold_slug(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}pct", value as i64)
    } else {
        format!("{}pct", value.to_string().replace('.', "p"))
    }
}

pub fn signal_column(signal_mode: &str) -> Result<&'static str> {
    match signal_mode {
        "oversold" => Ok("pct_below_or_equal"),
        "overbought" => Ok("pct_above"),
        _ => bail!("Unsupported signal mode: {signal_mode}"),
    }
}

pub fn signal_summary(signal_mode: &str, threshold: f64, lookback: usize) -> Result<String> {
    match signal_mode {
        "oversold" => Ok(format!(
            "oversold when >= {threshold:.2}% of universe is at/below {lookback}-day SMA"
        )),
        "overbought" => Ok(format!(
            "overbought when >= {threshold:.2}% of universe is above {lookback}-day SMA"
        )),
        _ => bail!("Unsupported signal mode: {signal_mode}"),
    }
}

fn get_trigger_value(row: &BreadthRow, signal_mode: &str) -> f64 {
    match signal_mode {
        "oversold" => row.pct_below_or_equal,
        "overbought" => row.pct_above,
        _ => f64::NAN,
    }
}

fn default_analysis_start(end_date: NaiveDate, sessions: usize, lookback: usize) -> NaiveDate {
    let buffer_days = std::cmp::max(sessions * 2, lookback * 10) as i64;
    end_date - chrono::Duration::days(buffer_days)
}

fn load_preset_metadata(path: &Path) -> Result<(String, Vec<String>)> {
    #[derive(Deserialize)]
    struct Payload {
        name: Option<String>,
        tickers: Option<Vec<String>>,
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading preset {}", path.display()))?;
    let payload: Payload = serde_json::from_str(&content)
        .with_context(|| format!("parsing preset {}", path.display()))?;
    let name = payload
        .name
        .unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().to_string());
    let tickers = match payload.tickers {
        Some(ref t) if !t.is_empty() => t.iter().map(|s| s.to_uppercase()).collect(),
        _ => bail!("Preset {} has no tickers", path.display()),
    };
    Ok((name, tickers))
}

// ---------------------------------------------------------------------------
// HTTP client (used for NASDAQ membership API only)
// ---------------------------------------------------------------------------

fn build_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------
// NASDAQ membership API
// ---------------------------------------------------------------------------

/// Fetch official Nasdaq index memberships for specific trade dates.
/// Includes retry logic with exponential back-off for rate-limited /
/// dropped connections, and a small delay between requests to be polite.
fn fetch_nasdaq_memberships(
    client: &reqwest::blocking::Client,
    snapshot_dates: &[NaiveDate],
    symbol: &str,
    time_of_day: &str,
) -> Result<BTreeMap<NaiveDate, HashSet<String>>> {
    let mut memberships: BTreeMap<NaiveDate, HashSet<String>> = BTreeMap::new();
    let max_retries: usize = 4;

    for (idx, &trade_date) in snapshot_dates.iter().enumerate() {
        let trade_date_str = format!("{}T00:00:00.000", trade_date);

        let mut members = HashSet::new();
        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let backoff = std::time::Duration::from_millis(500 * (1 << attempt));
                tracing::warn!(
                    "Retrying membership fetch for {} (attempt {}/{}) after {:?}",
                    trade_date,
                    attempt + 1,
                    max_retries + 1,
                    backoff,
                );
                std::thread::sleep(backoff);
            }

            let send_result = client
                .post(NASDAQ_WEIGHTING_URL)
                .form(&[
                    ("id", symbol),
                    ("tradeDate", &trade_date_str),
                    ("timeOfDay", time_of_day),
                ])
                .send();

            let resp = match send_result {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(
                        anyhow::anyhow!(e)
                            .context(format!("fetching NASDAQ memberships for {trade_date}")),
                    );
                    continue;
                }
            };

            if let Err(e) = resp.error_for_status_ref() {
                last_err = Some(
                    anyhow::anyhow!(e)
                        .context(format!("NASDAQ membership API error for {trade_date}")),
                );
                continue;
            }

            let payload: serde_json::Value = match resp.json() {
                Ok(v) => v,
                Err(e) => {
                    last_err = Some(
                        anyhow::anyhow!(e)
                            .context(format!("parsing membership JSON for {trade_date}")),
                    );
                    continue;
                }
            };

            let aa_data = payload
                .get("aaData")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            for row in &aa_data {
                if let Some(sym) = row.get("Symbol").and_then(|s| s.as_str()) {
                    if !sym.is_empty() {
                        members.insert(sym.to_uppercase());
                    }
                }
            }

            last_err = None;
            break;
        }

        if let Some(err) = last_err {
            return Err(err);
        }

        // Skip snapshot dates that return no members (holidays, weekends,
        // or dates before the API's historical coverage).
        if members.is_empty() {
            tracing::warn!("Snapshot date {} returned 0 members — skipping", trade_date);
            continue;
        }

        tracing::info!(
            "[{}/{}] {} → {} members",
            idx + 1,
            snapshot_dates.len(),
            trade_date,
            members.len()
        );
        memberships.insert(trade_date, members);

        // Small delay between requests to avoid rate-limiting.
        if idx + 1 < snapshot_dates.len() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    Ok(memberships)
}

// ---------------------------------------------------------------------------
// Membership expansion
// ---------------------------------------------------------------------------

/// Expand dated snapshot memberships across all trade dates.
fn expand_snapshot_memberships(
    trade_dates: &[NaiveDate],
    snapshots: &BTreeMap<NaiveDate, HashSet<String>>,
) -> Result<BTreeMap<NaiveDate, HashSet<String>>> {
    if snapshots.is_empty() {
        bail!("snapshots must be non-empty");
    }

    let ordered_snapshots: Vec<(NaiveDate, &HashSet<String>)> =
        snapshots.iter().map(|(d, s)| (*d, s)).collect();

    let mut expanded: BTreeMap<NaiveDate, HashSet<String>> = BTreeMap::new();
    let mut pointer: usize = 0;

    for &trade_date in trade_dates {
        while pointer + 1 < ordered_snapshots.len()
            && ordered_snapshots[pointer + 1].0 <= trade_date
        {
            pointer += 1;
        }
        expanded.insert(trade_date, ordered_snapshots[pointer].1.clone());
    }

    Ok(expanded)
}

/// Use the same ticker set for every trade date.
fn build_static_memberships(
    trade_dates: &[NaiveDate],
    symbols: &[String],
) -> Result<BTreeMap<NaiveDate, HashSet<String>>> {
    if symbols.is_empty() {
        bail!("static universe symbol set must be non-empty");
    }
    let normalized: HashSet<String> = symbols.iter().map(|s| s.to_uppercase()).collect();
    let mut memberships = BTreeMap::new();
    for &date in trade_dates {
        memberships.insert(date, normalized.clone());
    }
    Ok(memberships)
}

// ---------------------------------------------------------------------------
// Membership change table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MembershipChange {
    trade_date: NaiveDate,
    added: Vec<String>,
    removed: Vec<String>,
}

fn build_membership_change_table(
    memberships: &BTreeMap<NaiveDate, HashSet<String>>,
) -> Vec<MembershipChange> {
    let mut changes = Vec::new();
    let mut previous: Option<&HashSet<String>> = None;

    for (date, current) in memberships {
        if let Some(prev) = previous {
            if current != prev {
                let mut added: Vec<String> = current.difference(prev).cloned().collect();
                let mut removed: Vec<String> = prev.difference(current).cloned().collect();
                added.sort();
                removed.sort();
                changes.push(MembershipChange {
                    trade_date: *date,
                    added,
                    removed,
                });
            }
        }
        previous = Some(current);
    }

    changes
}

// ---------------------------------------------------------------------------
// Point-in-time breadth computation
// ---------------------------------------------------------------------------

/// Compute breadth using a date-specific membership set for each session.
fn compute_point_in_time_breadth(
    price_panel: &BTreeMap<String, Vec<(NaiveDate, f64)>>,
    memberships: &BTreeMap<NaiveDate, HashSet<String>>,
    lookback: usize,
) -> Result<Vec<BreadthRow>> {
    if lookback == 0 {
        bail!("lookback must be positive");
    }

    // Build per-symbol sorted price vectors and fast date->price lookup
    let mut symbol_prices: HashMap<String, Vec<(NaiveDate, f64)>> = HashMap::new();
    for (sym, data) in price_panel {
        let mut sorted_data = data.clone();
        sorted_data.sort_by_key(|(d, _)| *d);
        symbol_prices.insert(sym.to_uppercase(), sorted_data);
    }

    // Pre-compute SMA for each symbol
    let mut sma_map: HashMap<(NaiveDate, String), f64> = HashMap::new();
    for (sym, data) in &symbol_prices {
        for i in 0..data.len() {
            if i + 1 >= lookback {
                let sum: f64 = data[i + 1 - lookback..=i].iter().map(|(_, p)| p).sum();
                sma_map.insert((data[i].0, sym.clone()), sum / lookback as f64);
            }
        }
    }

    // Build a fast date->price lookup per symbol
    let mut price_map: HashMap<(NaiveDate, String), f64> = HashMap::new();
    for (sym, data) in &symbol_prices {
        for (d, p) in data {
            price_map.insert((*d, sym.clone()), *p);
        }
    }

    // Collect all trade dates from memberships
    let mut result = Vec::new();
    for (&date, members) in memberships {
        let universe_size = members.len();
        let mut eligible_count = 0;
        let mut above_count = 0;

        for sym in members {
            let sym_upper = sym.to_uppercase();
            let key = (date, sym_upper.clone());
            if let (Some(&price), Some(&sma)) = (price_map.get(&key), sma_map.get(&key)) {
                eligible_count += 1;
                if price > sma {
                    above_count += 1;
                }
            }
        }

        let below_count = eligible_count - above_count;
        let unavailable_count = universe_size - eligible_count;

        let (pct_above, pct_below) = if eligible_count > 0 {
            (
                above_count as f64 / eligible_count as f64 * 100.0,
                below_count as f64 / eligible_count as f64 * 100.0,
            )
        } else {
            (f64::NAN, f64::NAN)
        };

        result.push(BreadthRow {
            trade_date: date,
            eligible_count,
            above_count,
            below_or_equal_count: below_count,
            unavailable_count,
            pct_above,
            pct_below_or_equal: pct_below,
        });
    }

    result.sort_by_key(|r| r.trade_date);
    Ok(result)
}

/// Select trailing N sessions through the requested end date.
fn select_trailing(
    breadth: &[BreadthRow],
    end_date: NaiveDate,
    sessions: usize,
) -> Result<Vec<BreadthRow>> {
    if sessions == 0 {
        bail!("sessions must be positive");
    }
    if breadth.is_empty() {
        bail!("No eligible breadth observations on or before {end_date}");
    }

    let filtered: Vec<&BreadthRow> = breadth
        .iter()
        .filter(|r| r.trade_date <= end_date && r.eligible_count > 0)
        .collect();

    if filtered.is_empty() {
        bail!("No eligible breadth observations on or before {end_date}");
    }

    if filtered.last().unwrap().trade_date != end_date {
        let latest = filtered.last().unwrap().trade_date;
        bail!(
            "Requested end date {} is not present in the data; latest available date is {}",
            end_date,
            latest
        );
    }

    let start = filtered.len().saturating_sub(sessions);
    Ok(filtered[start..].iter().map(|r| (*r).clone()).collect())
}

// ---------------------------------------------------------------------------
// Forward return computation
// ---------------------------------------------------------------------------

/// Compute forward returns for a single asset's close series.
fn compute_forward_returns_for_asset(
    close_series: &[(NaiveDate, f64)],
    horizons: &[(String, usize)],
) -> BTreeMap<NaiveDate, Vec<(String, f64)>> {
    let n = close_series.len();
    let mut result: BTreeMap<NaiveDate, Vec<(String, f64)>> = BTreeMap::new();

    for i in 0..n {
        let mut horizon_returns = Vec::new();
        for (label, steps) in horizons {
            if i + steps < n {
                let ret = close_series[i + steps].1 / close_series[i].1 - 1.0;
                horizon_returns.push((label.clone(), ret));
            }
        }
        result.insert(close_series[i].0, horizon_returns);
    }

    result
}

// ---------------------------------------------------------------------------
// Forward return summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ForwardReturnRow {
    asset: String,
    horizon: String,
    signals: usize,
    observations: usize,
    mean_return_pct: f64,
    median_return_pct: f64,
    positive_rate_pct: f64,
    // Risk / performance metrics
    sharpe: f64,
    sortino: f64,
    max_drawdown_pct: f64,
    var_95_pct: f64,
    cvar_95_pct: f64,
    best_trade_pct: f64,
    worst_trade_pct: f64,
    std_dev_pct: f64,
    skewness: f64,
    kurtosis: f64,
    profit_factor: f64,
    avg_win_pct: f64,
    avg_loss_pct: f64,
    // Cumulative strategy return
    cumulative_return_pct: f64,
}

/// Compute risk/performance metrics from a vector of per-trade returns.
fn compute_trade_metrics(realized: &[f64]) -> TradeMetrics {
    let n = realized.len();
    if n == 0 {
        return TradeMetrics::nan();
    }

    let mean = realized.iter().sum::<f64>() / n as f64;

    // Sorted copy for percentile calculations
    let mut sorted = realized.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Median
    let median = if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    };

    // Positive rate
    let winners: Vec<f64> = realized.iter().copied().filter(|&r| r > 0.0).collect();
    let losers: Vec<f64> = realized.iter().copied().filter(|&r| r < 0.0).collect();
    let positive_rate = winners.len() as f64 / n as f64 * 100.0;

    // Standard deviation (sample, ddof=1)
    let std_dev = if n > 1 {
        let var = realized.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        var.sqrt()
    } else {
        0.0
    };

    // Downside deviation (target = 0)
    let downside_dev = if n > 1 {
        let dvar = realized.iter().map(|r| r.min(0.0).powi(2)).sum::<f64>() / (n - 1) as f64;
        dvar.sqrt()
    } else {
        0.0
    };

    // Sharpe (annualize by sqrt of trades-per-year; use raw mean/std for trade-level)
    let sharpe = if std_dev > 1e-15 { mean / std_dev } else { 0.0 };

    // Sortino
    let sortino = if downside_dev > 1e-15 {
        mean / downside_dev
    } else {
        0.0
    };

    // Max drawdown from compounding equity curve
    let mut equity = Vec::with_capacity(n + 1);
    equity.push(1.0);
    for r in realized {
        equity.push(equity.last().unwrap() * (1.0 + r));
    }
    let mut peak = equity[0];
    let mut max_dd: f64 = 0.0;
    for &val in &equity {
        if val > peak {
            peak = val;
        }
        let dd = (peak - val) / peak;
        if dd > max_dd {
            max_dd = dd;
        }
    }

    // Cumulative return
    let cumulative = *equity.last().unwrap() / equity[0] - 1.0;

    // VaR 95 (5th percentile)
    let var_95 = percentile_linear(&sorted, 5.0);

    // CVaR 95 (expected shortfall: mean of returns <= VaR)
    let tail: Vec<f64> = sorted.iter().copied().filter(|&r| r <= var_95).collect();
    let cvar_95 = if tail.is_empty() {
        var_95
    } else {
        tail.iter().sum::<f64>() / tail.len() as f64
    };

    // Best / worst
    let best = *sorted.last().unwrap();
    let worst = sorted[0];

    // Skewness
    let skewness = if n > 2 && std_dev > 1e-15 {
        let m3 = realized
            .iter()
            .map(|r| ((r - mean) / std_dev).powi(3))
            .sum::<f64>();
        m3 * n as f64 / ((n - 1) as f64 * (n - 2) as f64)
    } else {
        0.0
    };

    // Excess kurtosis
    let kurtosis = if n > 3 && std_dev > 1e-15 {
        let m4 = realized
            .iter()
            .map(|r| ((r - mean) / std_dev).powi(4))
            .sum::<f64>();
        let nf = n as f64;
        let k = (nf * (nf + 1.0)) / ((nf - 1.0) * (nf - 2.0) * (nf - 3.0)) * m4
            - 3.0 * (nf - 1.0).powi(2) / ((nf - 2.0) * (nf - 3.0));
        k
    } else {
        0.0
    };

    // Profit factor = gross wins / gross losses
    let gross_wins: f64 = winners.iter().sum();
    let gross_losses: f64 = losers.iter().map(|r| r.abs()).sum();
    let profit_factor = if gross_losses > 1e-15 {
        gross_wins / gross_losses
    } else if gross_wins > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };

    // Average win / average loss
    let avg_win = if winners.is_empty() {
        0.0
    } else {
        winners.iter().sum::<f64>() / winners.len() as f64
    };
    let avg_loss = if losers.is_empty() {
        0.0
    } else {
        losers.iter().sum::<f64>() / losers.len() as f64
    };

    TradeMetrics {
        mean,
        median,
        positive_rate,
        std_dev,
        sharpe,
        sortino,
        max_drawdown: max_dd,
        var_95,
        cvar_95,
        best,
        worst,
        skewness,
        kurtosis,
        profit_factor,
        avg_win,
        avg_loss,
        cumulative,
        _equity_curve: equity,
    }
}

struct TradeMetrics {
    mean: f64,
    median: f64,
    positive_rate: f64,
    std_dev: f64,
    sharpe: f64,
    sortino: f64,
    max_drawdown: f64,
    var_95: f64,
    cvar_95: f64,
    best: f64,
    worst: f64,
    skewness: f64,
    kurtosis: f64,
    profit_factor: f64,
    avg_win: f64,
    avg_loss: f64,
    cumulative: f64,
    _equity_curve: Vec<f64>,
}

impl TradeMetrics {
    fn nan() -> Self {
        Self {
            mean: f64::NAN,
            median: f64::NAN,
            positive_rate: f64::NAN,
            std_dev: f64::NAN,
            sharpe: f64::NAN,
            sortino: f64::NAN,
            max_drawdown: f64::NAN,
            var_95: f64::NAN,
            cvar_95: f64::NAN,
            best: f64::NAN,
            worst: f64::NAN,
            skewness: f64::NAN,
            kurtosis: f64::NAN,
            profit_factor: f64::NAN,
            avg_win: f64::NAN,
            avg_loss: f64::NAN,
            cumulative: f64::NAN,
            _equity_curve: Vec::new(),
        }
    }
}

/// Numpy-compatible linear interpolation percentile.
fn percentile_linear(sorted: &[f64], pct: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    if n == 1 {
        return sorted[0];
    }
    let rank = pct / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = rank - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

fn summarize_signal_forward_returns(
    trailing_breadth: &[BreadthRow],
    forward_prices: &BTreeMap<String, Vec<(NaiveDate, f64)>>,
    signal_mode: &str,
    threshold: f64,
    horizons: &[(String, usize)],
) -> Result<(Vec<BreadthRow>, Vec<ForwardReturnRow>)> {
    if !(0.0..=100.0).contains(&threshold) {
        bail!("signal threshold must be between 0 and 100");
    }

    // Find triggered rows
    let triggered: Vec<BreadthRow> = trailing_breadth
        .iter()
        .filter(|row| get_trigger_value(row, signal_mode) >= threshold)
        .cloned()
        .collect();

    if triggered.is_empty() {
        bail!(
            "No breadth observations matched {} >= {}",
            signal_column(signal_mode)?,
            threshold
        );
    }

    let trigger_dates: HashSet<NaiveDate> = triggered.iter().map(|r| r.trade_date).collect();

    let mut rows = Vec::new();
    for (asset, close_series) in forward_prices {
        if close_series.is_empty() {
            continue;
        }

        let forward = compute_forward_returns_for_asset(close_series, horizons);

        for (horizon_label, _) in horizons {
            let mut realized: Vec<f64> = Vec::new();
            for &tdate in &trigger_dates {
                if let Some(horizon_returns) = forward.get(&tdate) {
                    for (label, ret) in horizon_returns {
                        if label == horizon_label {
                            realized.push(*ret);
                        }
                    }
                }
            }

            let observations = realized.len();
            let m = compute_trade_metrics(&realized);

            rows.push(ForwardReturnRow {
                asset: asset.clone(),
                horizon: horizon_label.clone(),
                signals: triggered.len(),
                observations,
                mean_return_pct: m.mean * 100.0,
                median_return_pct: m.median * 100.0,
                positive_rate_pct: m.positive_rate,
                sharpe: m.sharpe,
                sortino: m.sortino,
                max_drawdown_pct: m.max_drawdown * 100.0,
                var_95_pct: m.var_95 * 100.0,
                cvar_95_pct: m.cvar_95 * 100.0,
                best_trade_pct: m.best * 100.0,
                worst_trade_pct: m.worst * 100.0,
                std_dev_pct: m.std_dev * 100.0,
                skewness: m.skewness,
                kurtosis: m.kurtosis,
                profit_factor: m.profit_factor,
                avg_win_pct: m.avg_win * 100.0,
                avg_loss_pct: m.avg_loss * 100.0,
                cumulative_return_pct: m.cumulative * 100.0,
            });
        }
    }

    Ok((triggered, rows))
}

// ---------------------------------------------------------------------------
// Universe resolution
// ---------------------------------------------------------------------------

struct UniverseResolution {
    label: String,
    memberships: BTreeMap<NaiveDate, HashSet<String>>,
    changes: Vec<MembershipChange>,
    all_symbols: Vec<String>,
}

fn resolve_universe(
    config: &BreadthWashoutConfig,
    trade_dates: &[NaiveDate],
    client: &reqwest::blocking::Client,
) -> Result<UniverseResolution> {
    match config.universe_mode.as_str() {
        "official-index" => {
            let index_symbol = config
                .index_symbol
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("official-index mode requires index_symbol"))?;

            let snapshot_dates: Vec<NaiveDate> = config
                .membership_snapshot_dates
                .iter()
                .map(|s| s.parse::<NaiveDate>())
                .collect::<Result<Vec<_>, _>>()?;

            let snapshots = fetch_nasdaq_memberships(
                client,
                &snapshot_dates,
                index_symbol,
                &config.membership_time_of_day,
            )?;

            let memberships = expand_snapshot_memberships(trade_dates, &snapshots)?;
            let changes = build_membership_change_table(&snapshots);

            // Collect all symbols across all snapshots
            let mut all_symbols: BTreeSet<String> = BTreeSet::new();
            for members in snapshots.values() {
                all_symbols.extend(members.iter().cloned());
            }

            Ok(UniverseResolution {
                label: config.universe_label.clone(),
                memberships,
                changes,
                all_symbols: all_symbols.into_iter().collect(),
            })
        }
        "preset" => {
            let preset_path = config
                .preset_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("preset universe mode requires preset_path"))?;
            let (_name, tickers) = load_preset_metadata(Path::new(preset_path))?;
            let memberships = build_static_memberships(trade_dates, &tickers)?;
            let label = config.universe_label.clone();
            Ok(UniverseResolution {
                label,
                memberships,
                changes: Vec::new(),
                all_symbols: tickers,
            })
        }
        "tickers" => {
            if config.explicit_tickers.is_empty() {
                bail!("tickers universe mode requires explicit_tickers");
            }
            let tickers: Vec<String> = config
                .explicit_tickers
                .iter()
                .map(|s| s.to_uppercase())
                .collect();
            let memberships = build_static_memberships(trade_dates, &tickers)?;
            Ok(UniverseResolution {
                label: config.universe_label.clone(),
                memberships,
                changes: Vec::new(),
                all_symbols: tickers,
            })
        }
        "all-stocks" => {
            let bronze_dir = config.bronze_dir.as_deref().map(Path::new);
            let tickers = discover_symbols(bronze_dir)?;
            if tickers.is_empty() {
                bail!("No symbols discovered in bronze directory");
            }
            let memberships = build_static_memberships(trade_dates, &tickers)?;
            Ok(UniverseResolution {
                label: config.universe_label.clone(),
                memberships,
                changes: Vec::new(),
                all_symbols: tickers,
            })
        }
        other => bail!("Unsupported universe mode: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Strategy results + reporting
// ---------------------------------------------------------------------------

struct StrategyResults {
    config: BreadthWashoutConfig,
    universe_label: String,
    trailing_breadth: Vec<BreadthRow>,
    target_row: BreadthRow,
    triggered: Vec<BreadthRow>,
    forward_summary: Vec<ForwardReturnRow>,
    membership_changes: Vec<MembershipChange>,
    missing_constituent_prices: Vec<String>,
    missing_forward_assets: Vec<String>,
}

fn format_strategy_report(results: &StrategyResults) -> String {
    let config = &results.config;
    let target_row = &results.target_row;
    let trailing = &results.trailing_breadth;

    let mut lines = vec![
        format!("Breadth Washout Strategy ({})", results.universe_label),
        format!(
            "Window: {} to {} ({} sessions)",
            trailing.first().unwrap().trade_date,
            trailing.last().unwrap().trade_date,
            trailing.len()
        ),
        format!(
            "Signal: {}",
            signal_summary(
                &config.signal_mode,
                config.signal_threshold,
                config.lookback
            )
            .unwrap_or_default()
        ),
        format!(
            "Forward returns: {} for {}",
            if config.adjusted_forward_returns {
                "adjusted close"
            } else {
                "close"
            },
            config.forward_assets.join(", ")
        ),
        String::new(),
        format!("As of {}", target_row.trade_date),
        format!(
            "Above {}-day SMA: {}",
            config.lookback, target_row.above_count
        ),
        format!(
            "At or below {}-day SMA: {} ({:.2}%)",
            config.lookback, target_row.below_or_equal_count, target_row.pct_below_or_equal
        ),
        format!("Signals in trailing window: {}", results.triggered.len()),
    ];

    if results.missing_constituent_prices.is_empty() {
        lines.push("Missing constituent price symbols: none".to_string());
    } else {
        lines.push(format!(
            "Missing constituent price symbols: {}",
            results.missing_constituent_prices.join(", ")
        ));
    }

    if !results.membership_changes.is_empty() {
        lines.push(String::new());
        lines.push("Membership change dates".to_string());

        // Compute column widths for alignment
        let mut max_date_w = "trade_date".len();
        let mut max_added_w = "added".len();
        let mut max_removed_w = "removed".len();
        for change in &results.membership_changes {
            let d = format!("{}", change.trade_date);
            let a = change.added.join(",");
            let r = change.removed.join(",");
            max_date_w = max_date_w.max(d.len());
            max_added_w = max_added_w.max(a.len());
            max_removed_w = max_removed_w.max(r.len());
        }

        lines.push(format!(
            "{:<width_d$} {:>width_a$} {:>width_r$}",
            "trade_date",
            "added",
            "removed",
            width_d = max_date_w,
            width_a = max_added_w,
            width_r = max_removed_w,
        ));
        for change in &results.membership_changes {
            lines.push(format!(
                "{:<width_d$} {:>width_a$} {:>width_r$}",
                change.trade_date,
                change.added.join(","),
                change.removed.join(","),
                width_d = max_date_w,
                width_a = max_added_w,
                width_r = max_removed_w,
            ));
        }
    }

    lines.push(String::new());
    lines.push("Forward-return summary".to_string());

    // Format forward return table
    lines.push(format!(
        "{:>5} {:>7}  {:>7}  {:>12}  {:>15}  {:>17}  {:>17}",
        "asset",
        "horizon",
        "signals",
        "observations",
        "mean_return_pct",
        "median_return_pct",
        "positive_rate_pct"
    ));
    for row in &results.forward_summary {
        lines.push(format!(
            "{:>5} {:>7}  {:>7}  {:>12}  {:>15.6}  {:>17.6}  {:>17.6}",
            row.asset,
            row.horizon,
            row.signals,
            row.observations,
            row.mean_return_pct,
            row.median_return_pct,
            row.positive_rate_pct,
        ));
    }

    // Risk metrics section — one block per asset
    let mut seen_assets: Vec<String> = Vec::new();
    for row in &results.forward_summary {
        if !seen_assets.contains(&row.asset) {
            seen_assets.push(row.asset.clone());
        }
    }

    for asset in &seen_assets {
        lines.push(String::new());
        lines.push(format!("Strategy metrics — {asset}"));
        lines.push(format!(
            "{:>7}  {:>10}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}  {:>9}  {:>10}  {:>8}  {:>9}  {:>10}",
            "horizon", "cumul_ret", "sharpe", "sortino", "max_dd", "var_95", "cvar_95",
            "std_dev", "win_rate", "prof_fact", "best_trade", "worst", "skewness", "kurtosis"
        ));
        for row in results.forward_summary.iter().filter(|r| r.asset == *asset) {
            let pf_str = if row.profit_factor.is_infinite() {
                "∞".to_string()
            } else {
                format!("{:.2}", row.profit_factor)
            };
            lines.push(format!(
                "{:>7}  {:>9.2}%  {:>8.3}  {:>8.3}  {:>7.2}%  {:>7.2}%  {:>7.2}%  {:>7.2}%  {:>7.1}%  {:>9}  {:>9.2}%  {:>7.2}%  {:>9.3}  {:>10.3}",
                row.horizon,
                row.cumulative_return_pct,
                row.sharpe,
                row.sortino,
                row.max_drawdown_pct,
                row.var_95_pct,
                row.cvar_95_pct,
                row.std_dev_pct,
                row.positive_rate_pct,
                pf_str,
                row.best_trade_pct,
                row.worst_trade_pct,
                row.skewness,
                row.kurtosis,
            ));
        }
    }

    lines.join("\n")
}

fn format_strategy_report_md(results: &StrategyResults) -> String {
    let config = &results.config;
    let target_row = &results.target_row;
    let trailing = &results.trailing_breadth;

    let mut lines = vec![
        format!("# Breadth Washout Strategy ({})", results.universe_label),
        String::new(),
        format!(
            "**Window:** {} to {} ({} sessions)  ",
            trailing.first().unwrap().trade_date,
            trailing.last().unwrap().trade_date,
            trailing.len()
        ),
        format!(
            "**Signal:** {}  ",
            signal_summary(
                &config.signal_mode,
                config.signal_threshold,
                config.lookback
            )
            .unwrap_or_default()
        ),
        format!(
            "**Forward returns:** {} for {}  ",
            if config.adjusted_forward_returns {
                "adjusted close"
            } else {
                "close"
            },
            config.forward_assets.join(", ")
        ),
        String::new(),
        format!("## As of {}", target_row.trade_date),
        String::new(),
        format!(
            "- **Above {}-day SMA:** {}",
            config.lookback, target_row.above_count
        ),
        format!(
            "- **At or below {}-day SMA:** {} ({:.2}%)",
            config.lookback, target_row.below_or_equal_count, target_row.pct_below_or_equal
        ),
        format!(
            "- **Signals in trailing window:** {}",
            results.triggered.len()
        ),
    ];

    if results.missing_constituent_prices.is_empty() {
        lines.push("- **Missing constituent prices:** none".to_string());
    } else {
        lines.push(format!(
            "- **Missing constituent prices:** {}",
            results.missing_constituent_prices.join(", ")
        ));
    }

    // Membership changes
    if !results.membership_changes.is_empty() {
        lines.push(String::new());
        lines.push("## Membership Changes".to_string());
        lines.push(String::new());
        lines.push("| Date | Added | Removed |".to_string());
        lines.push("|------|-------|---------|".to_string());
        for change in &results.membership_changes {
            lines.push(format!(
                "| {} | {} | {} |",
                change.trade_date,
                change.added.join(", "),
                change.removed.join(", "),
            ));
        }
    }

    // Forward return summary
    lines.push(String::new());
    lines.push("## Forward Returns".to_string());
    lines.push(String::new());
    lines.push("| Asset | Horizon | Signals | Obs | Mean % | Median % | Win Rate % |".to_string());
    lines.push("|-------|---------|---------|-----|--------|----------|------------|".to_string());
    for row in &results.forward_summary {
        lines.push(format!(
            "| {} | {} | {} | {} | {:.2} | {:.2} | {:.1} |",
            row.asset,
            row.horizon,
            row.signals,
            row.observations,
            row.mean_return_pct,
            row.median_return_pct,
            row.positive_rate_pct,
        ));
    }

    // Risk metrics per asset
    let mut seen_assets: Vec<String> = Vec::new();
    for row in &results.forward_summary {
        if !seen_assets.contains(&row.asset) {
            seen_assets.push(row.asset.clone());
        }
    }

    for asset in &seen_assets {
        lines.push(String::new());
        lines.push(format!("## Risk Metrics — {asset}"));
        lines.push(String::new());
        lines.push("| Horizon | Sharpe | Sortino | Max DD | VaR 95 | CVaR 95 | Std Dev | Best | Worst | Prof Factor |".to_string());
        lines.push("|---------|--------|---------|--------|--------|---------|---------|------|-------|-------------|".to_string());
        for row in results.forward_summary.iter().filter(|r| r.asset == *asset) {
            let pf_str = if row.profit_factor.is_infinite() {
                "∞".to_string()
            } else {
                format!("{:.2}", row.profit_factor)
            };
            lines.push(format!(
                "| {} | {:.3} | {:.3} | {:.2}% | {:.2}% | {:.2}% | {:.2}% | {:.2}% | {:.2}% | {} |",
                row.horizon,
                row.sharpe,
                row.sortino,
                row.max_drawdown_pct,
                row.var_95_pct,
                row.cvar_95_pct,
                row.std_dev_pct,
                row.best_trade_pct,
                row.worst_trade_pct,
                pf_str,
            ));
        }
    }

    lines.join("\n")
}

fn save_strategy_outputs(results: &StrategyResults) -> Result<Vec<(String, PathBuf)>> {
    let config = &results.config;
    let end_label = &config.end_date;
    let universe_slug = slugify(&results.universe_label);
    let signal_slug = format!(
        "{}_{}",
        config.signal_mode,
        threshold_slug(config.signal_threshold)
    );

    let out_dir = output_dir();
    std::fs::create_dir_all(&out_dir)?;

    let base = format!("{STRATEGY_SLUG}_{universe_slug}_{signal_slug}_{end_label}");

    // Summary CSV
    let summary_path = out_dir.join(format!("{base}_summary.csv"));
    {
        let mut wtr = std::fs::File::create(&summary_path)?;
        use std::io::Write;
        writeln!(
            wtr,
            "asset,horizon,signals,observations,mean_return_pct,median_return_pct,positive_rate_pct,\
             cumulative_return_pct,sharpe,sortino,max_drawdown_pct,var_95_pct,cvar_95_pct,\
             std_dev_pct,best_trade_pct,worst_trade_pct,skewness,kurtosis,profit_factor,\
             avg_win_pct,avg_loss_pct"
        )?;
        for row in &results.forward_summary {
            writeln!(
                wtr,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                row.asset,
                row.horizon,
                row.signals,
                row.observations,
                row.mean_return_pct,
                row.median_return_pct,
                row.positive_rate_pct,
                row.cumulative_return_pct,
                row.sharpe,
                row.sortino,
                row.max_drawdown_pct,
                row.var_95_pct,
                row.cvar_95_pct,
                row.std_dev_pct,
                row.best_trade_pct,
                row.worst_trade_pct,
                row.skewness,
                row.kurtosis,
                row.profit_factor,
                row.avg_win_pct,
                row.avg_loss_pct,
            )?;
        }
    }

    // Triggers CSV
    let trigger_path = out_dir.join(format!("{base}_triggers.csv"));
    {
        let mut wtr = std::fs::File::create(&trigger_path)?;
        use std::io::Write;
        writeln!(
            wtr,
            "trade_date,pct_above,pct_below_or_equal,above_count,below_or_equal_count,eligible_count,unavailable_count"
        )?;
        for row in &results.triggered {
            writeln!(
                wtr,
                "{},{:.6},{:.6},{},{},{},{}",
                row.trade_date,
                row.pct_above,
                row.pct_below_or_equal,
                row.above_count,
                row.below_or_equal_count,
                row.eligible_count,
                row.unavailable_count,
            )?;
        }
    }

    // Membership changes CSV
    let changes_path = out_dir.join(format!("{base}_membership_changes.csv"));
    {
        let mut wtr = std::fs::File::create(&changes_path)?;
        use std::io::Write;
        writeln!(wtr, "trade_date,added,removed")?;
        for change in &results.membership_changes {
            writeln!(
                wtr,
                "{},{},{}",
                change.trade_date,
                change.added.join(","),
                change.removed.join(","),
            )?;
        }
    }

    // Meta JSON
    let meta_path = out_dir.join(format!("{base}.json"));
    {
        let meta = serde_json::json!({
            "config": {
                "end_date": config.end_date,
                "sessions": config.sessions,
                "lookback": config.lookback,
                "signal_mode": config.signal_mode,
                "signal_threshold": config.signal_threshold,
                "universe_mode": config.universe_mode,
                "universe_label": config.universe_label,
                "index_symbol": config.index_symbol,
                "membership_time_of_day": config.membership_time_of_day,
                "membership_snapshot_dates": config.membership_snapshot_dates,
                "preset_path": config.preset_path,
                "explicit_tickers": config.explicit_tickers,
                "bronze_dir": config.bronze_dir,
                "forward_assets": config.forward_assets,
                "horizons": config.horizons,
                "adjusted_forward_returns": config.adjusted_forward_returns,
                "max_workers": config.max_workers,
            },
            "universe_label": results.universe_label,
            "target_row": {
                "trade_date": results.target_row.trade_date.to_string(),
                "eligible_count": results.target_row.eligible_count,
                "above_count": results.target_row.above_count,
                "below_or_equal_count": results.target_row.below_or_equal_count,
                "unavailable_count": results.target_row.unavailable_count,
                "pct_above": results.target_row.pct_above,
                "pct_below_or_equal": results.target_row.pct_below_or_equal,
            },
            "missing_constituent_prices": results.missing_constituent_prices,
            "missing_forward_assets": results.missing_forward_assets,
            "trigger_count": results.triggered.len(),
        });
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    }

    // Visualization JSON — rich data for the HTML dashboard
    let viz_path = out_dir.join(format!("{base}_viz.json"));
    {
        // Build per-asset metric rows
        let mut asset_metrics = serde_json::Map::new();
        for row in &results.forward_summary {
            let entry = asset_metrics
                .entry(row.asset.clone())
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            if let serde_json::Value::Array(arr) = entry {
                arr.push(serde_json::json!({
                    "horizon": row.horizon,
                    "signals": row.signals,
                    "observations": row.observations,
                    "mean_return_pct": row.mean_return_pct,
                    "median_return_pct": row.median_return_pct,
                    "positive_rate_pct": row.positive_rate_pct,
                    "cumulative_return_pct": row.cumulative_return_pct,
                    "sharpe": row.sharpe,
                    "sortino": row.sortino,
                    "max_drawdown_pct": row.max_drawdown_pct,
                    "var_95_pct": row.var_95_pct,
                    "cvar_95_pct": row.cvar_95_pct,
                    "std_dev_pct": row.std_dev_pct,
                    "best_trade_pct": row.best_trade_pct,
                    "worst_trade_pct": row.worst_trade_pct,
                    "skewness": row.skewness,
                    "kurtosis": row.kurtosis,
                    "profit_factor": row.profit_factor,
                    "avg_win_pct": row.avg_win_pct,
                    "avg_loss_pct": row.avg_loss_pct,
                }));
            }
        }

        // Triggered dates for timeline chart
        let trigger_points: Vec<serde_json::Value> = results
            .triggered
            .iter()
            .map(|r| {
                serde_json::json!({
                    "date": r.trade_date.to_string(),
                    "pct_below_or_equal": r.pct_below_or_equal,
                    "pct_above": r.pct_above,
                })
            })
            .collect();

        // Trailing breadth for the full time series chart
        let breadth_series: Vec<serde_json::Value> = results
            .trailing_breadth
            .iter()
            .map(|r| {
                serde_json::json!({
                    "date": r.trade_date.to_string(),
                    "pct_below_or_equal": r.pct_below_or_equal,
                    "pct_above": r.pct_above,
                    "eligible_count": r.eligible_count,
                })
            })
            .collect();

        // Membership changes
        let mem_changes: Vec<serde_json::Value> = results
            .membership_changes
            .iter()
            .map(|c| {
                serde_json::json!({
                    "date": c.trade_date.to_string(),
                    "added": c.added,
                    "removed": c.removed,
                })
            })
            .collect();

        let viz = serde_json::json!({
            "universe_label": results.universe_label,
            "signal_mode": config.signal_mode,
            "signal_threshold": config.signal_threshold,
            "lookback": config.lookback,
            "sessions": config.sessions,
            "end_date": config.end_date,
            "adjusted_forward_returns": config.adjusted_forward_returns,
            "forward_assets": config.forward_assets,
            "target_row": {
                "date": results.target_row.trade_date.to_string(),
                "above_count": results.target_row.above_count,
                "below_or_equal_count": results.target_row.below_or_equal_count,
                "pct_below_or_equal": results.target_row.pct_below_or_equal,
                "pct_above": results.target_row.pct_above,
                "eligible_count": results.target_row.eligible_count,
            },
            "asset_metrics": asset_metrics,
            "trigger_points": trigger_points,
            "breadth_series": breadth_series,
            "membership_changes": mem_changes,
            "missing_constituent_prices": results.missing_constituent_prices,
        });
        std::fs::write(&viz_path, serde_json::to_string_pretty(&viz)?)?;
    }

    Ok(vec![
        ("summary".to_string(), summary_path),
        ("triggers".to_string(), trigger_path),
        ("membership_changes".to_string(), changes_path),
        ("meta".to_string(), meta_path),
        ("viz".to_string(), viz_path),
    ])
}

// ---------------------------------------------------------------------------
// Core strategy runner
// ---------------------------------------------------------------------------

fn run_strategy(config: BreadthWashoutConfig) -> Result<StrategyResults> {
    let end_date = config.end_date.parse::<NaiveDate>()?;
    let analysis_start = default_analysis_start(end_date, config.sessions, config.lookback);
    let client = build_http_client();

    // Step 1: Get trading calendar from lead forward asset (warehouse parquet)
    tracing::info!(
        "Loading trading calendar from {} ...",
        config.forward_assets[0]
    );
    let wh = warehouse_root().ok();
    let (cal_panel, cal_missing) = load_price_panel(
        &config.forward_assets[..1].to_vec(),
        wh.as_deref(),
        Some(analysis_start),
        Some(end_date),
    )?;
    if !cal_missing.is_empty() {
        bail!(
            "Lead forward asset {} not found in warehouse",
            config.forward_assets[0]
        );
    }
    let calendar = cal_panel
        .get(&config.forward_assets[0])
        .cloned()
        .unwrap_or_default();
    if calendar.is_empty() {
        bail!("Could not determine a trading calendar from the lead forward asset");
    }
    let trade_dates: Vec<NaiveDate> = calendar.iter().map(|(d, _)| *d).collect();

    // Step 2: Resolve universe memberships
    tracing::info!("Resolving universe memberships ...");
    let universe = resolve_universe(&config, &trade_dates, &client)?;

    // Step 3: Load constituent prices from warehouse parquet
    tracing::info!(
        "Loading constituent prices for {} symbols from warehouse ...",
        universe.all_symbols.len()
    );
    let (constituent_prices, missing_constituent_prices) = load_price_panel(
        &universe.all_symbols,
        wh.as_deref(),
        Some(analysis_start),
        Some(end_date),
    )?;

    // Step 4: Compute breadth
    tracing::info!("Computing point-in-time breadth ...");
    let breadth =
        compute_point_in_time_breadth(&constituent_prices, &universe.memberships, config.lookback)?;
    let trailing_breadth = select_trailing(&breadth, end_date, config.sessions)?;

    let target_row = trailing_breadth
        .iter()
        .find(|r| r.trade_date == end_date)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("end date not found in trailing breadth data"))?;

    // Step 5: Load forward-return asset prices from warehouse parquet
    let fwd_start = trailing_breadth.first().unwrap().trade_date;
    tracing::info!(
        "Loading forward-return prices for {} from warehouse ...",
        config.forward_assets.join(", ")
    );
    let (forward_prices, missing_forward_assets) = load_price_panel(
        &config.forward_assets,
        wh.as_deref(),
        Some(fwd_start),
        Some(end_date),
    )?;

    // Step 6: Summarize forward returns conditioned on signals
    tracing::info!("Summarizing forward returns ...");
    let (triggered, forward_summary) = summarize_signal_forward_returns(
        &trailing_breadth,
        &forward_prices,
        &config.signal_mode,
        config.signal_threshold,
        &config.horizons,
    )?;

    Ok(StrategyResults {
        config,
        universe_label: universe.label,
        trailing_breadth,
        target_row,
        triggered,
        forward_summary,
        membership_changes: universe.changes,
        missing_constituent_prices,
        missing_forward_assets,
    })
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// CLI arguments for the breadth-washout strategy.
#[derive(Debug, clap::Args)]
pub struct BreadthWashoutArgs {
    #[arg(
        long,
        default_value = "2026-03-11",
        help = "Signal evaluation end date"
    )]
    pub end_date: String,

    #[arg(long, default_value_t = 252, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(long, default_value_t = 5, help = "Breadth SMA lookback")]
    pub lookback: usize,

    #[arg(long, default_value = "oversold", help = "Breadth signal mode")]
    pub signal_mode: String,

    #[arg(long, help = "Generic trigger threshold percent")]
    pub threshold: Option<f64>,

    #[arg(long, default_value_t = 65.0, help = "Oversold threshold alias")]
    pub min_pct_below: f64,

    #[arg(long, default_value = "ndx100", help = "Named universe")]
    pub universe: String,

    #[arg(long, help = "Custom preset JSON path")]
    pub preset: Option<String>,

    #[arg(long, num_args = 1.., help = "Explicit ticker list")]
    pub tickers: Option<Vec<String>>,

    #[arg(long, help = "Report label for universe")]
    pub universe_label: Option<String>,

    #[arg(long, default_value = "EOD", help = "Index membership snapshot timing")]
    pub membership_time_of_day: String,

    #[arg(long, help = "Membership snapshot dates")]
    pub snapshot_date: Option<Vec<String>>,

    #[arg(long, help = "Bronze dir override for all-stocks mode")]
    pub bronze_dir: Option<String>,

    #[arg(long, num_args = 1.., default_values_t = vec!["SPY".to_string(), "SPXL".to_string()], help = "Forward-return assets")]
    pub assets: Vec<String>,

    #[arg(long, help = "Forward horizon e.g. 1w=5")]
    pub horizon: Option<Vec<String>>,

    #[arg(long, help = "Use raw close for forward returns")]
    pub price_returns: bool,

    #[arg(long, default_value_t = 12, help = "Concurrent fetch workers")]
    pub max_workers: usize,
}

fn parse_horizons(values: &Option<Vec<String>>) -> Result<Vec<(String, usize)>> {
    match values {
        None => Ok(DEFAULT_FORWARD_HORIZONS
            .iter()
            .map(|(l, s)| (l.to_string(), *s))
            .collect()),
        Some(vals) => {
            let mut parsed = Vec::new();
            for value in vals {
                let parts: Vec<&str> = value.splitn(2, '=').collect();
                if parts.len() != 2 {
                    bail!("Invalid horizon '{value}'; expected label=periods");
                }
                let label = parts[0].to_string();
                let periods: usize = parts[1]
                    .parse()
                    .with_context(|| format!("Invalid horizon periods in '{value}'"))?;
                parsed.push((label, periods));
            }
            Ok(parsed)
        }
    }
}

fn build_config_from_args(args: &BreadthWashoutArgs) -> Result<BreadthWashoutConfig> {
    let signal_threshold = args.threshold.unwrap_or_else(|| {
        if args.signal_mode == "oversold" {
            args.min_pct_below
        } else {
            70.0
        }
    });

    let horizons = parse_horizons(&args.horizon)?;

    // Explicit tickers override everything
    if let Some(ref tickers) = args.tickers {
        return Ok(BreadthWashoutConfig {
            end_date: args.end_date.clone(),
            sessions: args.sessions,
            lookback: args.lookback,
            signal_mode: args.signal_mode.clone(),
            signal_threshold,
            universe_mode: "tickers".to_string(),
            universe_label: args
                .universe_label
                .clone()
                .unwrap_or_else(|| "tickers".to_string()),
            index_symbol: None,
            membership_time_of_day: args.membership_time_of_day.clone(),
            membership_snapshot_dates: Vec::new(),
            preset_path: None,
            explicit_tickers: tickers.iter().map(|s| s.to_uppercase()).collect(),
            bronze_dir: None,
            forward_assets: args.assets.iter().map(|s| s.to_uppercase()).collect(),
            horizons,
            adjusted_forward_returns: !args.price_returns,
            max_workers: args.max_workers,
        });
    }

    // Preset overrides named universe
    if let Some(ref preset) = args.preset {
        let (preset_name, _) = load_preset_metadata(Path::new(preset))?;
        return Ok(BreadthWashoutConfig {
            end_date: args.end_date.clone(),
            sessions: args.sessions,
            lookback: args.lookback,
            signal_mode: args.signal_mode.clone(),
            signal_threshold,
            universe_mode: "preset".to_string(),
            universe_label: args.universe_label.clone().unwrap_or(preset_name),
            index_symbol: None,
            membership_time_of_day: args.membership_time_of_day.clone(),
            membership_snapshot_dates: Vec::new(),
            preset_path: Some(preset.clone()),
            explicit_tickers: Vec::new(),
            bronze_dir: None,
            forward_assets: args.assets.iter().map(|s| s.to_uppercase()).collect(),
            horizons,
            adjusted_forward_returns: !args.price_returns,
            max_workers: args.max_workers,
        });
    }

    // Named universe lookup
    let universes = named_universes();
    let named = universes
        .iter()
        .find(|(name, _)| *name == args.universe)
        .map(|(_, u)| u)
        .ok_or_else(|| anyhow::anyhow!("Unknown universe: {}", args.universe))?;

    let preset_path = named.preset_name.map(|name| {
        presets_dir()
            .join(format!("{name}.json"))
            .to_string_lossy()
            .to_string()
    });

    let snapshot_dates = match &args.snapshot_date {
        Some(dates) => dates.clone(),
        None => DEFAULT_NDX_SNAPSHOT_DATES
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };

    Ok(BreadthWashoutConfig {
        end_date: args.end_date.clone(),
        sessions: args.sessions,
        lookback: args.lookback,
        signal_mode: args.signal_mode.clone(),
        signal_threshold,
        universe_mode: named.mode.to_string(),
        universe_label: args
            .universe_label
            .clone()
            .unwrap_or_else(|| named.label.to_string()),
        index_symbol: named.index_symbol.map(|s| s.to_string()),
        membership_time_of_day: args.membership_time_of_day.clone(),
        membership_snapshot_dates: snapshot_dates,
        preset_path,
        explicit_tickers: Vec::new(),
        bronze_dir: args.bronze_dir.clone(),
        forward_assets: args.assets.iter().map(|s| s.to_uppercase()).collect(),
        horizons,
        adjusted_forward_returns: !args.price_returns,
        max_workers: args.max_workers,
    })
}

/// Run the breadth washout strategy.
pub fn run(args: &BreadthWashoutArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let config = build_config_from_args(args)?;
    let results = run_strategy(config)?;
    let paths = save_strategy_outputs(&results)?;

    if json_mode {
        let output = serde_json::json!({
            "strategy": "breadth-washout",
            "universe_label": results.universe_label,
            "target_row": {
                "trade_date": results.target_row.trade_date.to_string(),
                "eligible_count": results.target_row.eligible_count,
                "above_count": results.target_row.above_count,
                "below_or_equal_count": results.target_row.below_or_equal_count,
                "unavailable_count": results.target_row.unavailable_count,
                "pct_above": results.target_row.pct_above,
                "pct_below_or_equal": results.target_row.pct_below_or_equal,
            },
            "trigger_count": results.triggered.len(),
            "forward_summary": results.forward_summary,
            "missing_constituent_prices": results.missing_constituent_prices,
            "missing_forward_assets": results.missing_forward_assets,
            "files": paths.iter().map(|(label, path)| {
                serde_json::json!({"label": label, "path": path.display().to_string()})
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string(&output)?);
    } else if fmt == OutputFormat::Md {
        println!("{}", format_strategy_report_md(&results));
    } else {
        println!("{}", format_strategy_report(&results));
        println!("\nFiles");
        for (label, path) in &paths {
            println!("{}: {}", label, path.display());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_column() {
        assert_eq!(signal_column("oversold").unwrap(), "pct_below_or_equal");
        assert_eq!(signal_column("overbought").unwrap(), "pct_above");
        assert!(signal_column("invalid").is_err());
    }

    #[test]
    fn test_signal_summary() {
        let s = signal_summary("oversold", 65.0, 5).unwrap();
        assert!(s.contains("65.00%"));
        assert!(s.contains("5-day SMA"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("NDX 100"), "ndx-100");
        assert_eq!(slugify("all-stocks"), "all-stocks");
    }

    #[test]
    fn test_threshold_slug() {
        assert_eq!(threshold_slug(65.0), "65pct");
        assert_eq!(threshold_slug(65.5), "65p5pct");
    }

    #[test]
    fn test_expand_snapshot_memberships() {
        let mut snapshots = BTreeMap::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        snapshots.insert(
            d1,
            vec!["AAPL", "MSFT"]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
        );
        snapshots.insert(
            d2,
            vec!["AAPL", "GOOG"]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
        );

        let dates = vec![
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        ];

        let expanded = expand_snapshot_memberships(&dates, &snapshots).unwrap();

        // Before d2: should use d1 membership
        assert!(expanded[&dates[0]].contains("MSFT"));
        assert!(!expanded[&dates[0]].contains("GOOG"));

        // At and after d2: should use d2 membership
        assert!(expanded[&dates[2]].contains("GOOG"));
        assert!(!expanded[&dates[2]].contains("MSFT"));
    }

    #[test]
    fn test_build_static_memberships() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
        ];
        let symbols = vec!["AAPL".to_string(), "msft".to_string()];
        let memberships = build_static_memberships(&dates, &symbols).unwrap();
        assert_eq!(memberships.len(), 2);
        for (_, members) in &memberships {
            assert!(members.contains("AAPL"));
            assert!(members.contains("MSFT"));
        }
    }

    #[test]
    fn test_build_membership_change_table() {
        let mut memberships = BTreeMap::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        memberships.insert(
            d1,
            vec!["AAPL", "MSFT"].into_iter().map(String::from).collect(),
        );
        memberships.insert(
            d2,
            vec!["AAPL", "MSFT"].into_iter().map(String::from).collect(),
        );
        memberships.insert(
            d3,
            vec!["AAPL", "GOOG"].into_iter().map(String::from).collect(),
        );

        let changes = build_membership_change_table(&memberships);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].trade_date, d3);
        assert_eq!(changes[0].added, vec!["GOOG"]);
        assert_eq!(changes[0].removed, vec!["MSFT"]);
    }

    #[test]
    fn test_compute_point_in_time_breadth() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();

        let mut prices = BTreeMap::new();
        prices.insert(
            "AAPL".to_string(),
            vec![(d1, 100.0), (d2, 102.0), (d3, 98.0)],
        );
        prices.insert(
            "MSFT".to_string(),
            vec![(d1, 200.0), (d2, 198.0), (d3, 196.0)],
        );

        let mut memberships = BTreeMap::new();
        let members: HashSet<String> = vec!["AAPL", "MSFT"].into_iter().map(String::from).collect();
        memberships.insert(d1, members.clone());
        memberships.insert(d2, members.clone());
        memberships.insert(d3, members);

        // lookback=1 means each day's SMA is just the current price
        let breadth = compute_point_in_time_breadth(&prices, &memberships, 1).unwrap();

        // With lookback=1, SMA == price, so price > SMA is never true
        // All should be below_or_equal
        assert_eq!(breadth.len(), 3);
        for row in &breadth {
            assert_eq!(row.above_count, 0);
            assert_eq!(row.below_or_equal_count, 2);
        }
    }

    #[test]
    fn test_get_trigger_value() {
        let row = BreadthRow {
            trade_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            eligible_count: 100,
            above_count: 40,
            below_or_equal_count: 60,
            unavailable_count: 0,
            pct_above: 40.0,
            pct_below_or_equal: 60.0,
        };
        assert_eq!(get_trigger_value(&row, "oversold"), 60.0);
        assert_eq!(get_trigger_value(&row, "overbought"), 40.0);
    }

    #[test]
    fn test_parse_horizons_default() {
        let horizons = parse_horizons(&None).unwrap();
        assert_eq!(horizons.len(), 4);
        assert_eq!(horizons[0], ("1d".to_string(), 1));
    }

    #[test]
    fn test_parse_horizons_custom() {
        let vals = Some(vec!["1w=5".to_string(), "2w=10".to_string()]);
        let horizons = parse_horizons(&vals).unwrap();
        assert_eq!(horizons.len(), 2);
        assert_eq!(horizons[0], ("1w".to_string(), 5));
        assert_eq!(horizons[1], ("2w".to_string(), 10));
    }

    #[test]
    fn test_parse_horizons_invalid() {
        let vals = Some(vec!["bad".to_string()]);
        assert!(parse_horizons(&vals).is_err());
    }

    #[test]
    fn test_forward_returns_for_asset() {
        let series: Vec<(NaiveDate, f64)> = (0..10)
            .map(|i| {
                (
                    NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                    100.0 + i as f64,
                )
            })
            .collect();
        let horizons = vec![("1d".to_string(), 1_usize), ("5d".to_string(), 5_usize)];
        let result = compute_forward_returns_for_asset(&series, &horizons);
        assert_eq!(result.len(), 10);

        // First date: 1d return = 101/100 - 1 = 0.01
        let first_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let first = &result[&first_date];
        assert!((first[0].1 - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_summarize_signal_forward_returns() {
        // Build 10 breadth rows where all are "triggered" (pct_below_or_equal >= 65)
        let breadth: Vec<BreadthRow> = (0..10)
            .map(|i| BreadthRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                eligible_count: 100,
                above_count: 30,
                below_or_equal_count: 70,
                unavailable_count: 0,
                pct_above: 30.0,
                pct_below_or_equal: 70.0,
            })
            .collect();

        // Build forward prices
        let mut forward_prices = BTreeMap::new();
        let spy_series: Vec<(NaiveDate, f64)> = (0..20)
            .map(|i| {
                (
                    NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                    100.0 + i as f64,
                )
            })
            .collect();
        forward_prices.insert("SPY".to_string(), spy_series);

        let horizons = vec![("1d".to_string(), 1_usize)];
        let (triggered, summary) = summarize_signal_forward_returns(
            &breadth,
            &forward_prices,
            "oversold",
            65.0,
            &horizons,
        )
        .unwrap();

        assert_eq!(triggered.len(), 10);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].asset, "SPY");
        assert_eq!(summary[0].horizon, "1d");
        assert_eq!(summary[0].signals, 10);
        assert!(summary[0].mean_return_pct > 0.0);
        // Verify new metrics are populated
        assert!(summary[0].sharpe.is_finite());
        assert!(summary[0].sortino.is_finite());
        assert!(summary[0].max_drawdown_pct >= 0.0);
        assert!(summary[0].std_dev_pct > 0.0);
        assert!(summary[0].cumulative_return_pct > 0.0);
        assert!(summary[0].profit_factor > 0.0);
        assert!(summary[0].best_trade_pct > 0.0);
    }

    #[test]
    fn test_default_analysis_start() {
        let end = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap();
        let start = default_analysis_start(end, 252, 5);
        // Should be end - max(504, 50) = end - 504 days
        let expected = end - chrono::Duration::days(504);
        assert_eq!(start, expected);
    }

    #[test]
    fn test_select_trailing() {
        let rows: Vec<BreadthRow> = (0..10)
            .map(|i| BreadthRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                eligible_count: 100,
                above_count: 50,
                below_or_equal_count: 50,
                unavailable_count: 0,
                pct_above: 50.0,
                pct_below_or_equal: 50.0,
            })
            .collect();

        let end = NaiveDate::from_ymd_opt(2024, 1, 11).unwrap();
        let trailing = select_trailing(&rows, end, 5).unwrap();
        assert_eq!(trailing.len(), 5);
        assert_eq!(trailing.last().unwrap().trade_date, end);
    }

    #[test]
    fn test_signal_summary_overbought() {
        let result = signal_summary("overbought", 70.0, 5).unwrap();
        assert!(result.contains("overbought"));
    }

    #[test]
    fn test_signal_summary_invalid() {
        let result = signal_summary("invalid", 50.0, 5);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_preset_metadata_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let preset = tmp.path().join("custom.json");
        std::fs::write(
            &preset,
            r#"{"name": "custom-set", "tickers": ["spy", "qqq"]}"#,
        )
        .unwrap();
        let (name, tickers) = load_preset_metadata(&preset).unwrap();
        assert_eq!(name, "custom-set");
        assert_eq!(tickers, vec!["SPY", "QQQ"]);
    }

    #[test]
    fn test_build_config_from_args_basic() {
        let args = BreadthWashoutArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            lookback: 5,
            signal_mode: "oversold".to_string(),
            threshold: None,
            min_pct_below: 65.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: None,
            universe_label: None,
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["SPY".to_string(), "SPXL".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 12,
        };
        let config = build_config_from_args(&args).unwrap();
        assert_eq!(config.signal_mode, "oversold");
        assert_eq!(config.signal_threshold, 65.0);
        assert_eq!(config.universe_mode, "preset");
        assert_eq!(config.universe_label, "ndx100");
    }

    #[test]
    fn test_build_config_explicit_tickers() {
        let args = BreadthWashoutArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            lookback: 5,
            signal_mode: "oversold".to_string(),
            threshold: None,
            min_pct_below: 65.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: Some(vec!["aapl".to_string(), "msft".to_string()]),
            universe_label: Some("tech-pair".to_string()),
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["SPY".to_string()],
            horizon: Some(vec!["2d=2".to_string()]),
            price_returns: true,
            max_workers: 4,
        };
        let config = build_config_from_args(&args).unwrap();
        assert_eq!(config.universe_mode, "tickers");
        assert_eq!(config.universe_label, "tech-pair");
        assert_eq!(config.explicit_tickers, vec!["AAPL", "MSFT"]);
    }

    #[test]
    fn test_build_config_overbought() {
        let args = BreadthWashoutArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            lookback: 5,
            signal_mode: "overbought".to_string(),
            threshold: Some(70.0),
            min_pct_below: 65.0,
            universe: "r2k".to_string(),
            preset: None,
            tickers: None,
            universe_label: None,
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["QQQ".to_string(), "TQQQ".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 8,
        };
        let config = build_config_from_args(&args).unwrap();
        assert_eq!(config.signal_mode, "overbought");
        assert_eq!(config.signal_threshold, 70.0);
        assert_eq!(config.forward_assets, vec!["QQQ", "TQQQ"]);
    }
}
