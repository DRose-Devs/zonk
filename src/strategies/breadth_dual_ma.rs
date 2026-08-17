/// Dual moving-average breadth strategy.
///
/// For each stock in the universe, checks two simultaneous conditions:
///   1. close < short-period MA (default 50-day) — short-term weakness
///   2. close > long-period MA  (default 200-day) — long-term uptrend intact
///
/// This identifies stocks that are pulling back within an ongoing uptrend.
/// The strategy computes what % of the universe satisfies BOTH conditions,
/// then triggers a signal when that % crosses a threshold.
///
/// Forward returns on specified assets (SPY, SPXL, QQQ, TQQQ, etc.) are
/// computed at each signal date with full risk metrics.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::config::{output_dir, presets_dir, warehouse_root};
use crate::data::discovery::discover_symbols;
use crate::data::readers::load_price_panel;

const STRATEGY_SLUG: &str = "breadth_dual_ma";
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

pub const DEFAULT_FORWARD_HORIZONS: &[(&str, usize)] =
    &[("1d", 1), ("1w", 5), ("1m", 21), ("3m", 63)];

// ---------------------------------------------------------------------------
// Dual-MA breadth row
// ---------------------------------------------------------------------------

/// Extended breadth row for dual-MA analysis.
///
/// Tracks four categories for each session:
///   - `below_short_above_long`: close < short MA AND close > long MA (pullback in uptrend)
///   - `below_both`:             close < short MA AND close <= long MA (bearish)
///   - `above_short`:            close >= short MA (not in pullback)
///   - `insufficient_data`:      stock lacks enough history for both MAs
#[derive(Debug, Clone, Serialize)]
pub struct DualMaBreadthRow {
    pub trade_date: NaiveDate,
    pub eligible_count: usize,
    pub below_short_above_long: usize,
    pub below_both: usize,
    pub above_short: usize,
    pub insufficient_data: usize,
    pub pct_below_short_above_long: f64,
    pub pct_below_both: f64,
    pub pct_above_short: f64,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DualMaConfig {
    pub end_date: String,
    pub sessions: usize,
    pub short_period: usize,
    pub long_period: usize,
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

// ---------------------------------------------------------------------------
// Named universes (same as breadth_washout)
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
// Helpers (shared with breadth_washout but kept local to avoid coupling)
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

fn default_analysis_start(end_date: NaiveDate, sessions: usize, long_period: usize) -> NaiveDate {
    let buffer_days = std::cmp::max(sessions * 2, long_period * 10) as i64;
    end_date - chrono::Duration::days(buffer_days)
}

fn load_preset_metadata(path: &Path) -> Result<(String, Vec<String>)> {
    #[derive(serde::Deserialize)]
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

        if idx + 1 < snapshot_dates.len() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    Ok(memberships)
}

// ---------------------------------------------------------------------------
// Membership helpers
// ---------------------------------------------------------------------------

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
// Dual-MA breadth computation
// ---------------------------------------------------------------------------

/// Compute dual-MA breadth: for each session, count stocks meeting each condition.
fn compute_dual_ma_breadth(
    price_panel: &BTreeMap<String, Vec<(NaiveDate, f64)>>,
    memberships: &BTreeMap<NaiveDate, HashSet<String>>,
    short_period: usize,
    long_period: usize,
) -> Result<Vec<DualMaBreadthRow>> {
    if short_period == 0 || long_period == 0 {
        bail!("MA periods must be positive");
    }
    if short_period >= long_period {
        bail!(
            "short_period ({}) must be less than long_period ({})",
            short_period,
            long_period
        );
    }

    // Build per-symbol sorted price vectors
    let mut symbol_prices: HashMap<String, Vec<(NaiveDate, f64)>> = HashMap::new();
    for (sym, data) in price_panel {
        let mut sorted_data = data.clone();
        sorted_data.sort_by_key(|(d, _)| *d);
        symbol_prices.insert(sym.to_uppercase(), sorted_data);
    }

    // Pre-compute both short and long SMAs for each symbol
    let mut short_sma_map: HashMap<(NaiveDate, String), f64> = HashMap::new();
    let mut long_sma_map: HashMap<(NaiveDate, String), f64> = HashMap::new();

    for (sym, data) in &symbol_prices {
        for i in 0..data.len() {
            if i + 1 >= short_period {
                let sum: f64 = data[i + 1 - short_period..=i].iter().map(|(_, p)| p).sum();
                short_sma_map.insert((data[i].0, sym.clone()), sum / short_period as f64);
            }
            if i + 1 >= long_period {
                let sum: f64 = data[i + 1 - long_period..=i].iter().map(|(_, p)| p).sum();
                long_sma_map.insert((data[i].0, sym.clone()), sum / long_period as f64);
            }
        }
    }

    // Build fast price lookup
    let mut price_map: HashMap<(NaiveDate, String), f64> = HashMap::new();
    for (sym, data) in &symbol_prices {
        for (d, p) in data {
            price_map.insert((*d, sym.clone()), *p);
        }
    }

    let mut result = Vec::new();
    for (&date, members) in memberships {
        let _universe_size = members.len();
        let mut eligible_count = 0;
        let mut below_short_above_long = 0;
        let mut below_both = 0;
        let mut above_short = 0;
        let mut insufficient_data = 0;

        for sym in members {
            let sym_upper = sym.to_uppercase();
            let key = (date, sym_upper.clone());

            let price = price_map.get(&key).copied();
            let short_sma = short_sma_map.get(&key).copied();
            let long_sma = long_sma_map.get(&key).copied();

            match (price, short_sma, long_sma) {
                (Some(p), Some(s_sma), Some(l_sma)) => {
                    eligible_count += 1;
                    if p >= s_sma {
                        above_short += 1;
                    } else if p > l_sma {
                        // close < short MA AND close > long MA
                        below_short_above_long += 1;
                    } else {
                        // close < short MA AND close <= long MA
                        below_both += 1;
                    }
                }
                (Some(_p), Some(_s_sma), None) => {
                    // Has short SMA but not enough data for long SMA
                    insufficient_data += 1;
                }
                _ => {
                    insufficient_data += 1;
                }
            }
        }

        let (pct_below_short_above_long, pct_below_both, pct_above_short) = if eligible_count > 0 {
            (
                below_short_above_long as f64 / eligible_count as f64 * 100.0,
                below_both as f64 / eligible_count as f64 * 100.0,
                above_short as f64 / eligible_count as f64 * 100.0,
            )
        } else {
            (f64::NAN, f64::NAN, f64::NAN)
        };

        result.push(DualMaBreadthRow {
            trade_date: date,
            eligible_count,
            below_short_above_long,
            below_both,
            above_short,
            insufficient_data,
            pct_below_short_above_long,
            pct_below_both,
            pct_above_short,
        });
    }

    result.sort_by_key(|r| r.trade_date);
    Ok(result)
}

/// Select trailing N sessions through the requested end date.
fn select_trailing(
    breadth: &[DualMaBreadthRow],
    end_date: NaiveDate,
    sessions: usize,
) -> Result<Vec<DualMaBreadthRow>> {
    if sessions == 0 {
        bail!("sessions must be positive");
    }
    if breadth.is_empty() {
        bail!("No eligible dual-MA breadth observations on or before {end_date}");
    }

    let filtered: Vec<&DualMaBreadthRow> = breadth
        .iter()
        .filter(|r| r.trade_date <= end_date && r.eligible_count > 0)
        .collect();

    if filtered.is_empty() {
        bail!("No eligible dual-MA breadth observations on or before {end_date}");
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
// Trade metrics (same as breadth_washout)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
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
        }
    }
}

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

fn compute_trade_metrics(realized: &[f64]) -> TradeMetrics {
    let n = realized.len();
    if n == 0 {
        return TradeMetrics::nan();
    }

    let mean = realized.iter().sum::<f64>() / n as f64;
    let mut sorted = realized.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median = if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    };

    let winners: Vec<f64> = realized.iter().copied().filter(|&r| r > 0.0).collect();
    let losers: Vec<f64> = realized.iter().copied().filter(|&r| r < 0.0).collect();
    let positive_rate = winners.len() as f64 / n as f64 * 100.0;

    let std_dev = if n > 1 {
        let var = realized.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        var.sqrt()
    } else {
        0.0
    };

    let downside_dev = if n > 1 {
        let dvar = realized.iter().map(|r| r.min(0.0).powi(2)).sum::<f64>() / (n - 1) as f64;
        dvar.sqrt()
    } else {
        0.0
    };

    let sharpe = if std_dev > 1e-15 { mean / std_dev } else { 0.0 };
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

    let cumulative = *equity.last().unwrap() / equity[0] - 1.0;
    let var_95 = percentile_linear(&sorted, 5.0);
    let tail: Vec<f64> = sorted.iter().copied().filter(|&r| r <= var_95).collect();
    let cvar_95 = if tail.is_empty() {
        var_95
    } else {
        tail.iter().sum::<f64>() / tail.len() as f64
    };

    let best = *sorted.last().unwrap();
    let worst = sorted[0];

    let skewness = if n > 2 && std_dev > 1e-15 {
        let m3 = realized
            .iter()
            .map(|r| ((r - mean) / std_dev).powi(3))
            .sum::<f64>();
        m3 * n as f64 / ((n - 1) as f64 * (n - 2) as f64)
    } else {
        0.0
    };

    let kurtosis = if n > 3 && std_dev > 1e-15 {
        let m4 = realized
            .iter()
            .map(|r| ((r - mean) / std_dev).powi(4))
            .sum::<f64>();
        let nf = n as f64;
        (nf * (nf + 1.0)) / ((nf - 1.0) * (nf - 2.0) * (nf - 3.0)) * m4
            - 3.0 * (nf - 1.0).powi(2) / ((nf - 2.0) * (nf - 3.0))
    } else {
        0.0
    };

    let gross_wins: f64 = winners.iter().sum();
    let gross_losses: f64 = losers.iter().map(|r| r.abs()).sum();
    let profit_factor = if gross_losses > 1e-15 {
        gross_wins / gross_losses
    } else if gross_wins > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };

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
    }
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
    cumulative_return_pct: f64,
}

fn summarize_signal_forward_returns(
    trailing_breadth: &[DualMaBreadthRow],
    forward_prices: &BTreeMap<String, Vec<(NaiveDate, f64)>>,
    threshold: f64,
    horizons: &[(String, usize)],
) -> Result<(Vec<DualMaBreadthRow>, Vec<ForwardReturnRow>)> {
    if !(0.0..=100.0).contains(&threshold) {
        bail!("signal threshold must be between 0 and 100");
    }

    // Trigger: % below short MA AND above long MA >= threshold
    let triggered: Vec<DualMaBreadthRow> = trailing_breadth
        .iter()
        .filter(|row| row.pct_below_short_above_long >= threshold)
        .cloned()
        .collect();

    if triggered.is_empty() {
        bail!(
            "No dual-MA breadth observations matched pct_below_short_above_long >= {}",
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
    config: &DualMaConfig,
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
            Ok(UniverseResolution {
                label: config.universe_label.clone(),
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
    config: DualMaConfig,
    universe_label: String,
    trailing_breadth: Vec<DualMaBreadthRow>,
    target_row: DualMaBreadthRow,
    triggered: Vec<DualMaBreadthRow>,
    forward_summary: Vec<ForwardReturnRow>,
    membership_changes: Vec<MembershipChange>,
    missing_constituent_prices: Vec<String>,
    missing_forward_assets: Vec<String>,
}

fn format_strategy_report(results: &StrategyResults) -> String {
    let config = &results.config;
    let target_row = &results.target_row;
    let trailing = &results.trailing_breadth;

    let mut out = String::new();
    out.push_str(&format!(
        "Dual-MA Breadth Strategy ({}d / {}d)\n",
        config.short_period, config.long_period
    ));
    out.push_str(&format!("Universe: {}\n", results.universe_label));
    out.push_str(&format!(
        "Window: {} to {} ({} sessions)\n",
        trailing.first().unwrap().trade_date,
        trailing.last().unwrap().trade_date,
        trailing.len()
    ));
    out.push_str(&format!(
        "Signal: >= {:.0}% of universe has close < {}d MA AND close > {}d MA\n",
        config.signal_threshold, config.short_period, config.long_period
    ));

    out.push_str(&format!("\nAs of {}\n", target_row.trade_date));
    out.push_str(&format!(
        "Below {}d MA & above {}d MA: {} ({:.2}%)\n",
        config.short_period,
        config.long_period,
        target_row.below_short_above_long,
        target_row.pct_below_short_above_long
    ));
    out.push_str(&format!(
        "Below both MAs: {} ({:.2}%)\n",
        target_row.below_both, target_row.pct_below_both
    ));
    out.push_str(&format!(
        "Above {}d MA: {} ({:.2}%)\n",
        config.short_period, target_row.above_short, target_row.pct_above_short
    ));
    out.push_str(&format!(
        "Insufficient data: {}\n",
        target_row.insufficient_data
    ));

    out.push_str(&format!(
        "\nTriggered sessions: {}\n",
        results.triggered.len()
    ));

    if !results.forward_summary.is_empty() {
        out.push_str(&format!(
            "\n{:>5} {:>7}  {:>7}  {:>13}  {:>16}  {:>17}\n",
            "asset", "horizon", "signals", "observations", "mean_return_pct", "positive_rate_pct"
        ));
        for row in &results.forward_summary {
            out.push_str(&format!(
                "{:>5} {:>7}  {:>7}  {:>13}  {:>16.6}  {:>17.6}\n",
                row.asset,
                row.horizon,
                row.signals,
                row.observations,
                row.mean_return_pct,
                row.positive_rate_pct
            ));
        }

        // Group by asset for detailed metrics
        let mut seen_assets: Vec<String> = Vec::new();
        for row in &results.forward_summary {
            if !seen_assets.contains(&row.asset) {
                seen_assets.push(row.asset.clone());
            }
        }

        for asset in &seen_assets {
            let asset_rows: Vec<&ForwardReturnRow> = results
                .forward_summary
                .iter()
                .filter(|r| &r.asset == asset)
                .collect();

            out.push_str(&format!("\nStrategy metrics — {}\n", asset));
            out.push_str(&format!(
                "{:>7}   {:>9}    {:>6}   {:>7}    {:>6}    {:>6}   {:>7}   {:>7}  {:>9}  {:>10}     {:>5}   {:>8}    {:>8}\n",
                "horizon", "cumul_ret", "sharpe", "sortino", "max_dd", "var_95",
                "cvar_95", "std_dev", "win_rate", "prof_fact", "best_trade", "worst", "skewness", 
            ));
            for r in &asset_rows {
                let pf_str = if r.profit_factor.is_infinite() {
                    "∞".to_string()
                } else {
                    format!("{:.2}", r.profit_factor)
                };
                out.push_str(&format!(
                    "{:>7}   {:>9.2}%    {:>6.3}   {:>7.3}    {:>5.2}%    {:>5.2}%   {:>6.2}%   {:>6.2}%  {:>8.1}%  {:>9}  {:>10.2}%     {:>5.2}%   {:>8.3}\n",
                    r.horizon, r.cumulative_return_pct, r.sharpe, r.sortino,
                    r.max_drawdown_pct, r.var_95_pct, r.cvar_95_pct, r.std_dev_pct,
                    r.positive_rate_pct, pf_str, r.best_trade_pct, r.worst_trade_pct,
                    r.skewness,
                ));
            }
        }
    }

    out
}

fn format_strategy_report_md(results: &StrategyResults) -> String {
    let config = &results.config;
    let target_row = &results.target_row;
    let trailing = &results.trailing_breadth;

    let mut lines = vec![
        format!(
            "# Dual-MA Breadth Strategy ({}d / {}d)",
            config.short_period, config.long_period
        ),
        String::new(),
        format!("**Universe:** {}  ", results.universe_label),
        format!(
            "**Window:** {} to {} ({} sessions)  ",
            trailing.first().unwrap().trade_date,
            trailing.last().unwrap().trade_date,
            trailing.len()
        ),
        format!(
            "**Signal:** ≥ {:.0}% of universe has close < {}d MA AND close > {}d MA  ",
            config.signal_threshold, config.short_period, config.long_period
        ),
        String::new(),
        format!("## As of {}", target_row.trade_date),
        String::new(),
        format!(
            "- **Below {}d MA & above {}d MA:** {} ({:.2}%)",
            config.short_period,
            config.long_period,
            target_row.below_short_above_long,
            target_row.pct_below_short_above_long
        ),
        format!(
            "- **Below both MAs:** {} ({:.2}%)",
            target_row.below_both, target_row.pct_below_both
        ),
        format!(
            "- **Above {}d MA:** {} ({:.2}%)",
            config.short_period, target_row.above_short, target_row.pct_above_short
        ),
        format!("- **Insufficient data:** {}", target_row.insufficient_data),
        format!("- **Triggered sessions:** {}", results.triggered.len()),
    ];

    if !results.forward_summary.is_empty() {
        lines.push(String::new());
        lines.push("## Forward Returns".to_string());
        lines.push(String::new());
        lines.push("| Asset | Horizon | Signals | Obs | Mean % | Win Rate % |".to_string());
        lines.push("|-------|---------|---------|-----|--------|------------|".to_string());
        for row in &results.forward_summary {
            lines.push(format!(
                "| {} | {} | {} | {} | {:.2} | {:.1} |",
                row.asset,
                row.horizon,
                row.signals,
                row.observations,
                row.mean_return_pct,
                row.positive_rate_pct,
            ));
        }

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
                    row.horizon, row.sharpe, row.sortino,
                    row.max_drawdown_pct, row.var_95_pct, row.cvar_95_pct,
                    row.std_dev_pct, row.best_trade_pct, row.worst_trade_pct, pf_str,
                ));
            }
        }
    }

    lines.join("\n")
}

fn save_strategy_outputs(results: &StrategyResults) -> Result<Vec<(String, PathBuf)>> {
    let out = output_dir();
    std::fs::create_dir_all(&out)?;

    let config = &results.config;
    let end_str = &config.end_date;
    let uni = slugify(&results.universe_label);
    let thr = threshold_slug(config.signal_threshold);
    let prefix = format!(
        "{STRATEGY_SLUG}_{uni}_{sp}d_{lp}d_{thr}_{end_str}",
        sp = config.short_period,
        lp = config.long_period,
    );

    let mut paths = Vec::new();

    // Summary CSV
    let summary_path = out.join(format!("{prefix}_summary.csv"));
    {
        use std::io::Write;
        let mut wtr = std::fs::File::create(&summary_path)?;
        writeln!(
            wtr,
            "asset,horizon,signals,observations,mean_return_pct,median_return_pct,\
             positive_rate_pct,cumulative_return_pct,sharpe,sortino,max_drawdown_pct,\
             var_95_pct,cvar_95_pct,best_trade_pct,worst_trade_pct,std_dev_pct,\
             skewness,kurtosis,profit_factor,avg_win_pct,avg_loss_pct"
        )?;
        for r in &results.forward_summary {
            let pf = if r.profit_factor.is_infinite() {
                "Inf".to_string()
            } else {
                format!("{:.6}", r.profit_factor)
            };
            writeln!(
                wtr,
                "{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{:.6},{:.6}",
                r.asset,
                r.horizon,
                r.signals,
                r.observations,
                r.mean_return_pct,
                r.median_return_pct,
                r.positive_rate_pct,
                r.cumulative_return_pct,
                r.sharpe,
                r.sortino,
                r.max_drawdown_pct,
                r.var_95_pct,
                r.cvar_95_pct,
                r.best_trade_pct,
                r.worst_trade_pct,
                r.std_dev_pct,
                r.skewness,
                r.kurtosis,
                pf,
                r.avg_win_pct,
                r.avg_loss_pct,
            )?;
        }
    }
    paths.push(("summary".to_string(), summary_path));

    // Triggers CSV
    let triggers_path = out.join(format!("{prefix}_triggers.csv"));
    {
        use std::io::Write;
        let mut wtr = std::fs::File::create(&triggers_path)?;
        writeln!(
            wtr,
            "trade_date,eligible_count,below_short_above_long,below_both,above_short,\
             insufficient_data,pct_below_short_above_long,pct_below_both,pct_above_short"
        )?;
        for row in &results.triggered {
            writeln!(
                wtr,
                "{},{},{},{},{},{},{:.6},{:.6},{:.6}",
                row.trade_date,
                row.eligible_count,
                row.below_short_above_long,
                row.below_both,
                row.above_short,
                row.insufficient_data,
                row.pct_below_short_above_long,
                row.pct_below_both,
                row.pct_above_short,
            )?;
        }
    }
    paths.push(("triggers".to_string(), triggers_path));

    // Membership changes CSV
    if !results.membership_changes.is_empty() {
        let changes_path = out.join(format!("{prefix}_membership_changes.csv"));
        {
            use std::io::Write;
            let mut wtr = std::fs::File::create(&changes_path)?;
            writeln!(wtr, "trade_date,added,removed")?;
            for change in &results.membership_changes {
                writeln!(
                    wtr,
                    "{},\"{}\",\"{}\"",
                    change.trade_date,
                    change.added.join(";"),
                    change.removed.join(";"),
                )?;
            }
        }
        paths.push(("membership_changes".to_string(), changes_path));
    }

    // Meta JSON
    let meta_path = out.join(format!("{prefix}.json"));
    {
        let meta = serde_json::json!({
            "strategy": "breadth-dual-ma",
            "short_period": config.short_period,
            "long_period": config.long_period,
            "signal_threshold": config.signal_threshold,
            "sessions": config.sessions,
            "end_date": config.end_date,
            "universe_label": results.universe_label,
            "forward_assets": config.forward_assets,
            "trigger_count": results.triggered.len(),
            "missing_constituent_prices": results.missing_constituent_prices,
            "missing_forward_assets": results.missing_forward_assets,
        });
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    }
    paths.push(("meta".to_string(), meta_path));

    // Viz JSON for dashboard consumption
    let viz_path = out.join(format!("{prefix}_viz.json"));
    {
        let mut asset_metrics: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
        for r in &results.forward_summary {
            let entry = asset_metrics.entry(r.asset.clone()).or_default();
            entry.push(serde_json::json!({
                "horizon": r.horizon,
                "signals": r.signals,
                "observations": r.observations,
                "mean_return_pct": r.mean_return_pct,
                "median_return_pct": r.median_return_pct,
                "positive_rate_pct": r.positive_rate_pct,
                "sharpe": r.sharpe,
                "sortino": r.sortino,
                "max_drawdown_pct": r.max_drawdown_pct,
                "var_95_pct": r.var_95_pct,
                "cvar_95_pct": r.cvar_95_pct,
                "best_trade_pct": r.best_trade_pct,
                "worst_trade_pct": r.worst_trade_pct,
                "std_dev_pct": r.std_dev_pct,
                "skewness": r.skewness,
                "kurtosis": r.kurtosis,
                "profit_factor": if r.profit_factor.is_infinite() { serde_json::Value::Null } else { serde_json::json!(r.profit_factor) },
                "avg_win_pct": r.avg_win_pct,
                "avg_loss_pct": r.avg_loss_pct,
                "cumulative_return_pct": r.cumulative_return_pct,
            }));
        }

        let breadth_series: Vec<serde_json::Value> = results
            .trailing_breadth
            .iter()
            .map(|r| {
                serde_json::json!({
                    "trade_date": r.trade_date.to_string(),
                    "eligible_count": r.eligible_count,
                    "below_short_above_long": r.below_short_above_long,
                    "below_both": r.below_both,
                    "above_short": r.above_short,
                    "pct_below_short_above_long": r.pct_below_short_above_long,
                    "pct_below_both": r.pct_below_both,
                    "pct_above_short": r.pct_above_short,
                })
            })
            .collect();

        let trigger_points: Vec<String> = results
            .triggered
            .iter()
            .map(|r| r.trade_date.to_string())
            .collect();

        let viz = serde_json::json!({
            "asset_metrics": asset_metrics,
            "breadth_series": breadth_series,
            "trigger_points": trigger_points,
        });
        std::fs::write(&viz_path, serde_json::to_string(&viz)?)?;
    }
    paths.push(("viz".to_string(), viz_path));

    Ok(paths)
}

// ---------------------------------------------------------------------------
// Core strategy runner
// ---------------------------------------------------------------------------

fn run_strategy(config: DualMaConfig) -> Result<StrategyResults> {
    let end_date = config.end_date.parse::<NaiveDate>()?;
    let analysis_start = default_analysis_start(end_date, config.sessions, config.long_period);
    let client = build_http_client();

    // Step 1: Trading calendar from lead forward asset (warehouse parquet)
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

    // Step 4: Compute dual-MA breadth
    tracing::info!(
        "Computing dual-MA breadth ({}d / {}d) ...",
        config.short_period,
        config.long_period
    );
    let breadth = compute_dual_ma_breadth(
        &constituent_prices,
        &universe.memberships,
        config.short_period,
        config.long_period,
    )?;
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

/// CLI arguments for the breadth-dual-ma strategy.
#[derive(Debug, clap::Args)]
pub struct BreadthDualMaArgs {
    #[arg(
        long,
        default_value = "2026-03-11",
        help = "Signal evaluation end date"
    )]
    pub end_date: String,

    #[arg(long, default_value_t = 252, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(long, default_value_t = 50, help = "Short MA period (e.g. 50-day)")]
    pub short_period: usize,

    #[arg(long, default_value_t = 200, help = "Long MA period (e.g. 200-day)")]
    pub long_period: usize,

    #[arg(
        long,
        default_value_t = 20.0,
        help = "Signal threshold: trigger when % below short MA and above long MA >= this"
    )]
    pub threshold: f64,

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

fn build_config_from_args(args: &BreadthDualMaArgs) -> Result<DualMaConfig> {
    let horizons = parse_horizons(&args.horizon)?;

    if args.short_period >= args.long_period {
        bail!(
            "short_period ({}) must be less than long_period ({})",
            args.short_period,
            args.long_period
        );
    }

    // Explicit tickers
    if let Some(ref tickers) = args.tickers {
        return Ok(DualMaConfig {
            end_date: args.end_date.clone(),
            sessions: args.sessions,
            short_period: args.short_period,
            long_period: args.long_period,
            signal_threshold: args.threshold,
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

    // Preset
    if let Some(ref preset) = args.preset {
        let (preset_name, _) = load_preset_metadata(Path::new(preset))?;
        return Ok(DualMaConfig {
            end_date: args.end_date.clone(),
            sessions: args.sessions,
            short_period: args.short_period,
            long_period: args.long_period,
            signal_threshold: args.threshold,
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

    // Named universe
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

    Ok(DualMaConfig {
        end_date: args.end_date.clone(),
        sessions: args.sessions,
        short_period: args.short_period,
        long_period: args.long_period,
        signal_threshold: args.threshold,
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

/// Run the dual-MA breadth strategy.
pub fn run(args: &BreadthDualMaArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let config = build_config_from_args(args)?;
    let results = run_strategy(config)?;
    let paths = save_strategy_outputs(&results)?;

    if json_mode {
        let output = serde_json::json!({
            "strategy": "breadth-dual-ma",
            "short_period": results.config.short_period,
            "long_period": results.config.long_period,
            "signal_threshold": results.config.signal_threshold,
            "sessions": results.config.sessions,
            "end_date": results.config.end_date,
            "universe_label": results.universe_label,
            "target_row": {
                "trade_date": results.target_row.trade_date.to_string(),
                "eligible_count": results.target_row.eligible_count,
                "below_short_above_long": results.target_row.below_short_above_long,
                "below_both": results.target_row.below_both,
                "above_short": results.target_row.above_short,
                "pct_below_short_above_long": results.target_row.pct_below_short_above_long,
                "pct_below_both": results.target_row.pct_below_both,
                "pct_above_short": results.target_row.pct_above_short,
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
    fn test_compute_dual_ma_breadth_basic() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        let d4 = NaiveDate::from_ymd_opt(2024, 1, 4).unwrap();
        let d5 = NaiveDate::from_ymd_opt(2024, 1, 5).unwrap();

        let mut prices = BTreeMap::new();
        // Stock A: steady uptrend, then pullback
        prices.insert(
            "A".to_string(),
            vec![
                (d1, 100.0),
                (d2, 102.0),
                (d3, 104.0),
                (d4, 106.0),
                (d5, 99.0),
            ],
        );
        // Stock B: steady decline
        prices.insert(
            "B".to_string(),
            vec![(d1, 100.0), (d2, 98.0), (d3, 96.0), (d4, 94.0), (d5, 92.0)],
        );

        let members: HashSet<String> = vec!["A", "B"].into_iter().map(String::from).collect();
        let mut memberships = BTreeMap::new();
        for &d in &[d1, d2, d3, d4, d5] {
            memberships.insert(d, members.clone());
        }

        // short=2, long=4
        let breadth = compute_dual_ma_breadth(&prices, &memberships, 2, 4).unwrap();
        assert!(!breadth.is_empty());
        // On d5 with short_period=2, long_period=4:
        //   Stock A: 2d SMA = (106+99)/2 = 102.5, 4d SMA = (102+104+106+99)/4 = 102.75
        //            close=99 < 102.5 (short) AND 99 < 102.75 (long) → below_both
        //   Stock B: 2d SMA = (94+92)/2 = 93, 4d SMA = (98+96+94+92)/4 = 95
        //            close=92 < 93 (short) AND 92 < 95 (long) → below_both
        let last = breadth.last().unwrap();
        assert_eq!(last.trade_date, d5);
        assert_eq!(last.eligible_count, 2);
    }

    #[test]
    fn test_compute_dual_ma_breadth_pullback_in_uptrend() {
        // Create a stock in a long-term uptrend with a short-term pullback
        let mut prices = BTreeMap::new();
        let mut memberships = BTreeMap::new();

        // 10 days of prices: big uptrend, then slight dip at end
        let dates: Vec<NaiveDate> = (0..10)
            .map(|i| NaiveDate::from_ymd_opt(2024, 1, 1 + i).unwrap())
            .collect();
        // Uptrend: 100, 105, 110, 115, 120, 125, 130, 135, 140, 128
        // Day 10 (128): 3d SMA = (135+140+128)/3 = 134.3, 5d SMA = (125+130+135+140+128)/5 = 131.6
        //   close=128 < 134.3 (short) AND 128 < 131.6 (long) → below_both, not pullback
        // But let's set up a proper case:
        let series_a: Vec<f64> = vec![
            90.0, 95.0, 100.0, 105.0, 110.0, 115.0, 120.0, 125.0, 130.0, 118.0,
        ];
        // Day 10: 3d SMA = (125+130+118)/3 = 124.3, 5d SMA = (110+115+120+125+130)/5 = 120
        //   Wait, we need 5d SMA at day 10, using days 6-10: (115+120+125+130+118)/5 = 121.6
        //   close=118 < 124.3 (short) AND 118 < 121.6 → below_both
        // Need a more extreme pullback in uptrend scenario:
        let series_b: Vec<f64> = vec![50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0, 78.0];
        // Day 10: short (3d) SMA = (85+90+78)/3 = 84.3, long (5d) SMA = (75+80+85+90+78)/5 = 81.6
        //   close=78 < 84.3 (short) BUT 78 < 81.6 → below_both
        // For a real pullback in uptrend, need close > long MA but < short MA
        // That means long MA needs to be lower. Use longer period = bigger lag.
        // Stock C: strong uptrend, small dip
        let series_c: Vec<f64> = vec![50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 90.0, 100.0, 88.0];
        // Day 10: short (2d) SMA = (100+88)/2 = 94, long (5d) SMA = (75+80+90+100+88)/5 = 86.6
        //   close=88 < 94 (below short) AND 88 > 86.6 (above long) → PULLBACK IN UPTREND ✓

        prices.insert(
            "C".to_string(),
            dates
                .iter()
                .zip(series_c.iter())
                .map(|(d, p)| (*d, *p))
                .collect(),
        );

        let members: HashSet<String> = vec!["C"].into_iter().map(String::from).collect();
        for &d in &dates {
            memberships.insert(d, members.clone());
        }

        let breadth = compute_dual_ma_breadth(&prices, &memberships, 2, 5).unwrap();
        let last = breadth.last().unwrap();
        assert_eq!(last.trade_date, dates[9]);
        assert_eq!(last.below_short_above_long, 1);
        assert_eq!(last.below_both, 0);
        assert_eq!(last.above_short, 0);
        assert!((last.pct_below_short_above_long - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_dual_ma_periods_validation() {
        let prices = BTreeMap::new();
        let memberships = BTreeMap::new();

        // short >= long should fail
        assert!(compute_dual_ma_breadth(&prices, &memberships, 50, 50).is_err());
        assert!(compute_dual_ma_breadth(&prices, &memberships, 200, 50).is_err());

        // zero periods should fail
        assert!(compute_dual_ma_breadth(&prices, &memberships, 0, 50).is_err());
        assert!(compute_dual_ma_breadth(&prices, &memberships, 50, 0).is_err());
    }

    #[test]
    fn test_select_trailing_dual_ma() {
        let rows: Vec<DualMaBreadthRow> = (0..10)
            .map(|i| DualMaBreadthRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                eligible_count: 100,
                below_short_above_long: 30,
                below_both: 20,
                above_short: 50,
                insufficient_data: 0,
                pct_below_short_above_long: 30.0,
                pct_below_both: 20.0,
                pct_above_short: 50.0,
            })
            .collect();

        let end = NaiveDate::from_ymd_opt(2024, 1, 11).unwrap();
        let trailing = select_trailing(&rows, end, 5).unwrap();
        assert_eq!(trailing.len(), 5);
        assert_eq!(trailing.last().unwrap().trade_date, end);
    }

    #[test]
    fn test_select_trailing_zero_sessions() {
        let result = select_trailing(&[], NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_threshold_slug_values() {
        assert_eq!(threshold_slug(40.0), "40pct");
        assert_eq!(threshold_slug(37.5), "37p5pct");
    }

    #[test]
    fn test_slugify_values() {
        assert_eq!(slugify("NDX 100"), "ndx-100");
        assert_eq!(slugify("dual-ma"), "dual-ma");
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
        let horizons = vec![("1d".to_string(), 1_usize)];
        let result = compute_forward_returns_for_asset(&series, &horizons);
        assert_eq!(result.len(), 10);
        let first_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        assert!((result[&first_date][0].1 - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_summarize_signal_forward_returns() {
        let breadth: Vec<DualMaBreadthRow> = (0..10)
            .map(|i| DualMaBreadthRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                eligible_count: 100,
                below_short_above_long: 50,
                below_both: 20,
                above_short: 30,
                insufficient_data: 0,
                pct_below_short_above_long: 50.0,
                pct_below_both: 20.0,
                pct_above_short: 30.0,
            })
            .collect();

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
        let (triggered, summary) =
            summarize_signal_forward_returns(&breadth, &forward_prices, 40.0, &horizons).unwrap();

        assert_eq!(triggered.len(), 10);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].asset, "SPY");
        assert_eq!(summary[0].signals, 10);
        assert!(summary[0].mean_return_pct > 0.0);
    }

    #[test]
    fn test_build_config_from_args_basic() {
        let args = BreadthDualMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            short_period: 50,
            long_period: 200,
            threshold: 20.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: None,
            universe_label: None,
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["SPY".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 12,
        };
        let config = build_config_from_args(&args).unwrap();
        assert_eq!(config.short_period, 50);
        assert_eq!(config.long_period, 200);
        assert_eq!(config.signal_threshold, 20.0);
    }

    #[test]
    fn test_build_config_invalid_periods() {
        let args = BreadthDualMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            short_period: 200,
            long_period: 50,
            threshold: 40.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: None,
            universe_label: None,
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["SPY".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 12,
        };
        assert!(build_config_from_args(&args).is_err());
    }

    #[test]
    fn test_build_config_explicit_tickers() {
        let args = BreadthDualMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            short_period: 50,
            long_period: 200,
            threshold: 40.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: Some(vec!["aapl".to_string(), "msft".to_string()]),
            universe_label: Some("tech".to_string()),
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["QQQ".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 12,
        };
        let config = build_config_from_args(&args).unwrap();
        assert_eq!(config.universe_mode, "tickers");
        assert_eq!(config.explicit_tickers, vec!["AAPL", "MSFT"]);
    }

    #[test]
    fn test_parse_horizons_default() {
        let horizons = parse_horizons(&None).unwrap();
        assert_eq!(horizons.len(), 4);
        assert_eq!(horizons[0], ("1d".to_string(), 1));
    }

    #[test]
    fn test_parse_horizons_custom() {
        let vals = Some(vec!["2d=2".to_string(), "1m=21".to_string()]);
        let horizons = parse_horizons(&vals).unwrap();
        assert_eq!(horizons.len(), 2);
        assert_eq!(horizons[1], ("1m".to_string(), 21));
    }

    #[test]
    fn test_parse_horizons_invalid() {
        let vals = Some(vec!["bad".to_string()]);
        assert!(parse_horizons(&vals).is_err());
    }

    #[test]
    fn test_expand_snapshot_memberships() {
        let mut snapshots = BTreeMap::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        snapshots.insert(d1, vec!["A", "B"].into_iter().map(String::from).collect());
        snapshots.insert(d2, vec!["A", "C"].into_iter().map(String::from).collect());

        let dates = vec![
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        ];
        let expanded = expand_snapshot_memberships(&dates, &snapshots).unwrap();
        assert!(expanded[&dates[0]].contains("B"));
        assert!(!expanded[&dates[0]].contains("C"));
        assert!(expanded[&dates[1]].contains("C"));
    }

    #[test]
    fn test_build_membership_change_table() {
        let mut memberships = BTreeMap::new();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        memberships.insert(d1, vec!["A", "B"].into_iter().map(String::from).collect());
        memberships.insert(d2, vec!["A", "C"].into_iter().map(String::from).collect());

        let changes = build_membership_change_table(&memberships);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].added, vec!["C"]);
        assert_eq!(changes[0].removed, vec!["B"]);
    }

    #[test]
    fn test_compute_trade_metrics_basic() {
        let returns = vec![0.01, 0.02, -0.005, 0.015, -0.01, 0.03];
        let m = compute_trade_metrics(&returns);
        assert!(m.mean > 0.0);
        assert!(m.sharpe > 0.0);
        assert!(m.positive_rate > 50.0);
        assert!(m.best > 0.0);
        assert!(m.worst < 0.0);
    }

    #[test]
    fn test_compute_trade_metrics_empty() {
        let m = compute_trade_metrics(&[]);
        assert!(m.mean.is_nan());
    }

    #[test]
    fn test_percentile_linear_basic() {
        let sorted = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile_linear(&sorted, 50.0) - 3.0).abs() < 0.01);
        assert!((percentile_linear(&sorted, 0.0) - 1.0).abs() < 0.01);
        assert!((percentile_linear(&sorted, 100.0) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_default_analysis_start() {
        let end = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap();
        let start = default_analysis_start(end, 252, 200);
        // Should be end - max(504, 2000) = end - 2000 days
        let expected = end - chrono::Duration::days(2000);
        assert_eq!(start, expected);
    }
}
