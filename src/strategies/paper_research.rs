/// Paper-driven trading strategy runner.
///
/// This strategy family is used by the autoresearch loop for web-sourced
/// hypotheses that are not yet encoded as native strategy families.
/// It applies a single-rule signal on close-price history and evaluates a
/// long-only equity curve against a buy-and-hold baseline.
use anyhow::{Result, bail};
use chrono::NaiveDate;

use num_format::ToFormattedString;
use serde::Serialize;

use std::collections::HashMap;

use crate::cli::OutputFormat;
use crate::data::readers::{load_ticker_ohlcv, load_vix_ohlcv, load_volatility_index_ohlcv};
use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::strategies::common::{
    JsonOutput, StrategyResult, build_json_annual_returns, buy_and_hold_equity,
    compute_strategy_metrics, format_annual_table, format_results_header, format_strategy_row,
};

const DEFAULT_CAPITAL: f64 = 1_000_000.0;
const DEFAULT_TTLYRS: f64 = 365.25;
const DEFAULT_END_DATE: &str = "2026-03-11";
const DEFAULT_SESSIONS: usize = 252;
const DEFAULT_FAST_WINDOW: usize = 12;
const DEFAULT_SLOW_WINDOW: usize = 40;
const DEFAULT_RSI_WINDOW: usize = 14;
const DEFAULT_VOL_WINDOW: usize = 20;

#[derive(Debug, Clone, Copy)]
enum ResearchRule {
    TrendMomentum,
    TrendPullback,
    RsiReversion,
    VolatilityRegime,
    VolSpread,
    MeanReversionFilter,
    VvixRegime,
}

impl ResearchRule {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "trend_momentum" => Some(Self::TrendMomentum),
            "trend_pullback" => Some(Self::TrendPullback),
            "rsi_reversion" => Some(Self::RsiReversion),
            "volatility_regime" => Some(Self::VolatilityRegime),
            "vol_spread" => Some(Self::VolSpread),
            "mean_reversion_filter" => Some(Self::MeanReversionFilter),
            "vvix_regime" => Some(Self::VvixRegime),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TrendMomentum => "trend_momentum",
            Self::TrendPullback => "trend_pullback",
            Self::RsiReversion => "rsi_reversion",
            Self::VolatilityRegime => "volatility_regime",
            Self::VolSpread => "vol_spread",
            Self::MeanReversionFilter => "mean_reversion_filter",
            Self::VvixRegime => "vvix_regime",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct PaperResearchArgs {
    #[arg(
        long,
        help = "Inclusive start date for scoring windows (YYYY-MM-DD). Overrides trailing-session start selection."
    )]
    pub start_date: Option<String>,

    #[arg(long, default_value = DEFAULT_END_DATE, help = "Inclusive end date for scoring windows (YYYY-MM-DD)")]
    pub end_date: String,

    #[arg(long, default_value_t = DEFAULT_SESSIONS, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(
        long,
        default_value = "SPY",
        help = "Asset to generate signals and trade"
    )]
    pub asset: String,

    #[arg(
        long,
        default_value = "trend_momentum",
        help = "Research rule: trend_momentum|trend_pullback|rsi_reversion|volatility_regime|vol_spread|mean_reversion_filter|vvix_regime"
    )]
    pub rule: String,

    #[arg(long, default_value_t = DEFAULT_FAST_WINDOW, help = "Primary lookback for MA and momentum signal rules")]
    pub fast_window: usize,

    #[arg(long, default_value_t = DEFAULT_SLOW_WINDOW, help = "Secondary MA lookback for trend rules")]
    pub slow_window: usize,

    #[arg(long, default_value_t = DEFAULT_RSI_WINDOW, help = "RSI lookback")]
    pub rsi_window: usize,

    #[arg(
        long,
        default_value_t = 35.0,
        help = "RSI oversold threshold for reversion candidates"
    )]
    pub rsi_oversold: f64,

    #[arg(
        long,
        default_value_t = 65.0,
        help = "RSI overbought threshold (not used unless strategy variant requires it)"
    )]
    pub rsi_overbought: f64,

    #[arg(long, default_value_t = DEFAULT_VOL_WINDOW, help = "Realized-volatility lookback")]
    pub vol_window: usize,

    #[arg(
        long,
        default_value_t = 0.45,
        help = "Volatility percentile cap [0,1] for volatility_regime; spread threshold for vol_spread (negative = snap-back)"
    )]
    pub vol_cap: f64,

    #[arg(
        long,
        default_value_t = 0.02,
        help = "Mean reversion entry threshold: go long when price is this fraction below fair value SMA"
    )]
    pub mr_entry_threshold: f64,

    #[arg(
        long,
        default_value_t = 200.0,
        help = "Mean reversion position scaling factor (reserved for fractional sizing)"
    )]
    pub mr_scale: f64,

    #[arg(
        long,
        default_value_t = 63,
        help = "VVIX rolling percentile lookback window (trading days)"
    )]
    pub vvix_window: usize,

    #[arg(
        long,
        default_value_t = 0.75,
        help = "VVIX percentile threshold; risk_off: long when below, contrarian: long when above"
    )]
    pub vvix_threshold: f64,

    #[arg(
        long,
        default_value = "risk_off",
        help = "VVIX mode: risk_off (long when VVIX low) | contrarian (long when VVIX high)"
    )]
    pub vvix_mode: String,

    #[arg(long, help = "Optional hypothesis id")]
    pub hypothesis_id: Option<String>,

    #[arg(
        long,
        help = "Include per-trade and per-bar equity audit data in JSON output"
    )]
    pub include_audit: bool,

    #[arg(long, default_value_t = 12, help = "Concurrent fetch workers")]
    pub max_workers: usize,

    #[arg(long, default_value_t = 2015, help = "Annual table start year")]
    pub start_year_table: i32,
}

#[derive(Debug, Clone, Serialize)]
struct AuditTrade {
    entry_date: String,
    exit_date: String,
    entry_price: f64,
    exit_price: f64,
    shares: i64,
    fee: f64,
    gross_pnl: f64,
    net_pnl: f64,
    equity_before: f64,
    equity_after: f64,
}

#[derive(Debug, Clone, Serialize)]
struct AuditEquityPoint {
    date: String,
    equity: f64,
    step: String,
    signal: bool,
    entry_date: Option<String>,
    exit_date: Option<String>,
    entry_price: Option<f64>,
    exit_price: Option<f64>,
    shares: Option<i64>,
    fee: Option<f64>,
    gross_pnl: Option<f64>,
    net_pnl: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct ExecutionAudit {
    strategy: String,
    asset: String,
    rule: String,
    requested_period_start: Option<String>,
    requested_period_end: String,
    actual_period_start: String,
    actual_period_end: String,
    capital: f64,
    fee_model: String,
    bar_count: usize,
    signal_count: usize,
    executed_trade_count: usize,
    beginning_equity: f64,
    ending_equity: f64,
    equity_trace: Vec<AuditEquityPoint>,
    trades: Vec<AuditTrade>,
}

fn moving_average(values: &[f64], end: usize, window: usize) -> Option<f64> {
    if window == 0 || end + 1 < window {
        return None;
    }
    let start = end + 1 - window;
    let mut sum = 0.0;
    for v in &values[start..=end] {
        if !v.is_finite() {
            return None;
        }
        sum += *v;
    }
    Some(sum / window as f64)
}

fn rolling_volatility(returns: &[f64], end: usize, window: usize) -> Option<f64> {
    if window < 2 || end + 1 < window {
        return None;
    }
    let start = end + 1 - window;
    let mut vals = Vec::new();
    for r in &returns[start..=end] {
        if !r.is_finite() {
            continue;
        }
        vals.push(*r);
    }
    if vals.len() < window {
        return None;
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (vals.len() as f64 - 1.0);
    let vol = var.sqrt() * DEFAULT_TTLYRS.sqrt();
    if vol.is_finite() { Some(vol) } else { None }
}

fn rolling_rsi(closes: &[f64], end: usize, window: usize) -> Option<f64> {
    if window < 2 || end < window {
        return None;
    }
    let start = end - window + 1;
    let mut gains = 0.0;
    let mut losses = 0.0;
    for idx in start..=end {
        let prev = closes[idx.saturating_sub(1)];
        let cur = closes[idx];
        if !cur.is_finite() || !prev.is_finite() {
            return None;
        }
        let d = cur - prev;
        if d > 0.0 {
            gains += d;
        } else {
            losses += -d;
        }
    }
    let avg_gain = gains / window as f64;
    let avg_loss = losses / window as f64;
    let rsi = if avg_loss <= 0.0 {
        100.0
    } else {
        let rs = avg_gain / avg_loss;
        100.0 - 100.0 / (1.0 + rs)
    };
    Some(rsi)
}

/// Compute annualized realized volatility from log returns using sqrt(252).
///
/// Returns the standard deviation of `log_returns[end+1-window..=end]` times sqrt(252).
/// Compatible with VIX annualization convention (trading days, not calendar days).
fn realized_vol_252(log_returns: &[f64], end: usize, window: usize) -> Option<f64> {
    if window < 2 || end + 1 < window {
        return None;
    }
    let start = end + 1 - window;
    let mut vals = Vec::with_capacity(window);
    for r in &log_returns[start..=end] {
        if !r.is_finite() {
            return None;
        }
        vals.push(*r);
    }
    if vals.len() < window {
        return None;
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (vals.len() as f64 - 1.0);
    let vol = var.sqrt() * (252.0_f64).sqrt();
    if vol.is_finite() { Some(vol) } else { None }
}

fn build_signal_mask(
    closes: &[f64],
    rule: ResearchRule,
    fast: usize,
    slow: usize,
    rsi_window: usize,
    rsi_oversold: f64,
    vol_window: usize,
    vol_cap: f64,
    vix_closes: Option<&[f64]>,
    mr_entry_threshold: f64,
    vvix_closes: Option<&[f64]>,
    vvix_window: usize,
    vvix_threshold: f64,
    vvix_contrarian: bool,
) -> Vec<bool> {
    let n = closes.len();

    // Length guard: if VIX data is provided, it must match closes length
    if let Some(vix) = vix_closes {
        assert_eq!(
            vix.len(),
            n,
            "vix_closes length ({}) must match closes length ({})",
            vix.len(),
            n
        );
    }
    if let Some(vvix) = vvix_closes {
        assert_eq!(
            vvix.len(),
            n,
            "vvix_closes length ({}) must match closes length ({})",
            vvix.len(),
            n
        );
    }

    // Rule-specific minimum lookback gate
    if matches!(rule, ResearchRule::VolSpread) {
        if n < vol_window + 1 {
            return vec![false; n];
        }
    } else if matches!(rule, ResearchRule::MeanReversionFilter) {
        if n < slow.saturating_add(1) {
            return vec![false; n];
        }
    } else if matches!(rule, ResearchRule::VvixRegime) {
        if n < vvix_window + 1 {
            return vec![false; n];
        }
    } else if n < slow.saturating_add(4) {
        return vec![false; n];
    }

    let returns: Vec<f64> = closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect();

    let vols: Vec<f64> = closes
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            rolling_volatility(&returns, idx.saturating_sub(1), vol_window).unwrap_or(f64::NAN)
        })
        .collect();
    let valid_vol: Vec<f64> = vols.iter().copied().filter(|v| v.is_finite()).collect();
    let mut vol_threshold = None;
    if valid_vol.len() >= 5 && (0.0..=1.0).contains(&vol_cap) {
        let mut quantile_values = valid_vol.clone();
        quantile_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((quantile_values.len() as f64 - 1.0) * vol_cap).round() as usize;
        vol_threshold = quantile_values.get(idx).copied();
    }

    (0..closes.len())
        .map(|idx| {
            let signal = match rule {
                ResearchRule::TrendMomentum => {
                    let short = moving_average(closes, idx, fast);
                    let long = moving_average(closes, idx, slow);
                    match (short, long) {
                        (Some(s), Some(l)) => s > l && closes[idx] > s,
                        _ => false,
                    }
                }
                ResearchRule::TrendPullback => {
                    let short = moving_average(closes, idx, fast);
                    let long = moving_average(closes, idx, slow);
                    match (short, long) {
                        (Some(s), Some(l)) => closes[idx] < s && closes[idx] > l,
                        _ => false,
                    }
                }
                ResearchRule::RsiReversion => rolling_rsi(closes, idx, rsi_window)
                    .map(|v| v < rsi_oversold)
                    .unwrap_or(false),
                ResearchRule::VolatilityRegime => {
                    let vol = vols.get(idx).copied().unwrap_or(f64::NAN);
                    if !vol.is_finite() {
                        return false;
                    }
                    if vol_threshold.is_none() {
                        false
                    } else {
                        vol <= vol_threshold.unwrap_or(f64::INFINITY)
                    }
                }
                ResearchRule::VolSpread => {
                    // Need at least vol_window returns to compute realized vol
                    if idx == 0 || idx.saturating_sub(1) + 1 < vol_window {
                        return false;
                    }
                    let realized =
                        match realized_vol_252(&returns, idx.saturating_sub(1), vol_window) {
                            Some(v) if v.is_finite() => v,
                            _ => return false,
                        };
                    let implied = match vix_closes {
                        Some(vix) => {
                            let v = vix.get(idx).copied().unwrap_or(f64::NAN);
                            if !v.is_finite() {
                                return false;
                            }
                            v / 100.0
                        }
                        None => return false,
                    };
                    let spread = (implied - realized) / realized.max(0.01);
                    if vol_cap >= 0.0 {
                        // VRP harvest: go long when VIX overstates realized
                        spread > vol_cap
                    } else {
                        // Snap-back: go long when realized overshoots implied
                        spread < vol_cap
                    }
                }
                ResearchRule::MeanReversionFilter => {
                    // Fair value = SMA(close, slow_window)
                    // Relative mispricing δ = (close - fair_value) / fair_value
                    // Go long when δ < -mr_entry_threshold (price below fair value)
                    let fair_value = moving_average(closes, idx, slow);
                    match fair_value {
                        Some(fv) if fv > 0.0 => {
                            let delta = (closes[idx] - fv) / fv;
                            delta < -mr_entry_threshold
                        }
                        _ => false,
                    }
                }
                ResearchRule::VvixRegime => {
                    // Compute rolling percentile rank of VVIX within lookback window
                    if idx < vvix_window {
                        return false;
                    }
                    let vvix_val = match vvix_closes {
                        Some(vvix) => {
                            let v = vvix.get(idx).copied().unwrap_or(f64::NAN);
                            if !v.is_finite() { return false; }
                            v
                        }
                        None => return false,
                    };
                    let vvix = vvix_closes.unwrap();
                    let window_start = idx.saturating_sub(vvix_window);
                    let mut count_below = 0usize;
                    let mut count_valid = 0usize;
                    for i in window_start..idx {
                        let v = vvix[i];
                        if v.is_finite() {
                            count_valid += 1;
                            if v <= vvix_val {
                                count_below += 1;
                            }
                        }
                    }
                    if count_valid < 2 {
                        return false;
                    }
                    let percentile_rank = count_below as f64 / count_valid as f64;
                    if vvix_contrarian {
                        // Paper's direct signal: long when VVIX is high (buy cheap vol uncertainty)
                        percentile_rank >= vvix_threshold
                    } else {
                        // Risk-off: long equity when VVIX is low (calm conditions)
                        percentile_rank < vvix_threshold
                    }
                }
            };
            signal && closes[idx].is_finite()
        })
        .collect()
}

fn simulate_strategy(closes: &[f64], mask: &[bool], capital: f64) -> Vec<f64> {
    let n = closes.len();
    if n < 2 {
        return vec![capital];
    }
    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    let mut current = capital;
    for i in 0..n {
        if i + 1 < n {
            if mask[i] {
                let shares = (current / closes[i]) as i64;
                if shares > 0 {
                    let fee = ibkr_roundtrip_cost(current, closes[i]);
                    let pnl = shares as f64 * (closes[i + 1] - closes[i]);
                    current = current + pnl - fee;
                }
            }
        }
        equity.push(current);
    }
    equity
}

fn indicator_buffer_days(max_lookback: usize) -> i64 {
    max_lookback
        .saturating_mul(6)
        .saturating_add(30)
        .try_into()
        .unwrap_or(i64::MAX)
}

fn evaluation_start_index(
    dates: &[NaiveDate],
    requested_start_date: Option<NaiveDate>,
    sessions: usize,
) -> Result<usize> {
    if dates.is_empty() {
        bail!("no dates available for evaluation");
    }

    if let Some(start_date) = requested_start_date {
        return dates
            .iter()
            .position(|date| *date >= start_date)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no trading bars available on or after requested start date {start_date}"
                )
            });
    }

    let start_idx = dates.len().saturating_sub(sessions);
    if start_idx == dates.len() {
        bail!(
            "insufficient rows for requested sessions {} (available {})",
            sessions,
            dates.len()
        );
    }
    Ok(start_idx)
}

fn build_execution_audit(
    asset: &str,
    rule: ResearchRule,
    dates: &[NaiveDate],
    closes: &[f64],
    mask: &[bool],
    capital: f64,
    requested_start_date: Option<NaiveDate>,
    requested_end_date: NaiveDate,
) -> ExecutionAudit {
    assert_eq!(
        dates.len(),
        closes.len(),
        "dates/closes length mismatch in audit build"
    );
    assert_eq!(
        closes.len(),
        mask.len(),
        "closes/mask length mismatch in audit build"
    );

    let mut current = capital;
    let mut trades = Vec::new();
    let mut equity_trace = Vec::with_capacity(dates.len().max(1));

    if let Some(first_date) = dates.first() {
        equity_trace.push(AuditEquityPoint {
            date: first_date.to_string(),
            equity: capital,
            step: "start".to_string(),
            signal: false,
            entry_date: None,
            exit_date: None,
            entry_price: None,
            exit_price: None,
            shares: None,
            fee: None,
            gross_pnl: None,
            net_pnl: None,
        });
    }

    for idx in 0..closes.len().saturating_sub(1) {
        let signal = mask[idx];
        let exit_date = dates[idx + 1].to_string();

        if signal {
            let shares = (current / closes[idx]) as i64;
            if shares > 0 {
                let fee = ibkr_roundtrip_cost(current, closes[idx]);
                let gross_pnl = shares as f64 * (closes[idx + 1] - closes[idx]);
                let net_pnl = gross_pnl - fee;
                let equity_before = current;
                current += net_pnl;

                trades.push(AuditTrade {
                    entry_date: dates[idx].to_string(),
                    exit_date: exit_date.clone(),
                    entry_price: closes[idx],
                    exit_price: closes[idx + 1],
                    shares,
                    fee,
                    gross_pnl,
                    net_pnl,
                    equity_before,
                    equity_after: current,
                });

                equity_trace.push(AuditEquityPoint {
                    date: exit_date,
                    equity: current,
                    step: "executed_trade".to_string(),
                    signal: true,
                    entry_date: Some(dates[idx].to_string()),
                    exit_date: Some(dates[idx + 1].to_string()),
                    entry_price: Some(closes[idx]),
                    exit_price: Some(closes[idx + 1]),
                    shares: Some(shares),
                    fee: Some(fee),
                    gross_pnl: Some(gross_pnl),
                    net_pnl: Some(net_pnl),
                });
                continue;
            }

            equity_trace.push(AuditEquityPoint {
                date: exit_date,
                equity: current,
                step: "signal_no_shares".to_string(),
                signal: true,
                entry_date: Some(dates[idx].to_string()),
                exit_date: Some(dates[idx + 1].to_string()),
                entry_price: Some(closes[idx]),
                exit_price: Some(closes[idx + 1]),
                shares: Some(0),
                fee: Some(0.0),
                gross_pnl: Some(0.0),
                net_pnl: Some(0.0),
            });
            continue;
        }

        equity_trace.push(AuditEquityPoint {
            date: exit_date,
            equity: current,
            step: "flat".to_string(),
            signal: false,
            entry_date: None,
            exit_date: None,
            entry_price: None,
            exit_price: None,
            shares: None,
            fee: None,
            gross_pnl: None,
            net_pnl: None,
        });
    }

    ExecutionAudit {
        strategy: "paper-research".to_string(),
        asset: asset.to_string(),
        rule: rule.as_str().to_string(),
        requested_period_start: requested_start_date.map(|date| date.to_string()),
        requested_period_end: requested_end_date.to_string(),
        actual_period_start: dates.first().map(ToString::to_string).unwrap_or_default(),
        actual_period_end: dates.last().map(ToString::to_string).unwrap_or_default(),
        capital,
        fee_model: "IBKR Tiered".to_string(),
        bar_count: dates.len(),
        signal_count: mask
            .iter()
            .take(closes.len().saturating_sub(1))
            .filter(|&&v| v)
            .count(),
        executed_trade_count: trades.len(),
        beginning_equity: capital,
        ending_equity: current,
        equity_trace,
        trades,
    }
}

/// Run a paper-derived research strategy.
pub fn run(args: &PaperResearchArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let quiet = json_mode || fmt == OutputFormat::Md;
    let asset = args.asset.to_uppercase();
    let rule = ResearchRule::from_str(&args.rule).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported rule '{}'; expected trend_momentum, trend_pullback, rsi_reversion, volatility_regime, vol_spread, mean_reversion_filter, vvix_regime",
            args.rule
        )
    })?;

    // Rule-specific vol_cap validation
    match rule {
        ResearchRule::VolatilityRegime => {
            if !(0.0..=1.0).contains(&args.vol_cap) {
                bail!(
                    "volatility_regime requires --vol-cap in [0.0, 1.0], got {}",
                    args.vol_cap
                );
            }
        }
        ResearchRule::VolSpread => {
            if !args.vol_cap.is_finite() {
                bail!(
                    "vol_spread requires a finite --vol-cap threshold, got {}",
                    args.vol_cap
                );
            }
        }
        ResearchRule::MeanReversionFilter => {
            if !args.mr_entry_threshold.is_finite() || args.mr_entry_threshold <= 0.0 {
                bail!(
                    "mean_reversion_filter requires --mr-entry-threshold > 0, got {}",
                    args.mr_entry_threshold
                );
            }
            if !args.mr_scale.is_finite() || args.mr_scale <= 0.0 {
                bail!(
                    "mean_reversion_filter requires --mr-scale > 0, got {}",
                    args.mr_scale
                );
            }
        }
        ResearchRule::VvixRegime => {
            if !(0.0..=1.0).contains(&args.vvix_threshold) {
                bail!(
                    "vvix_regime requires --vvix-threshold in [0.0, 1.0], got {}",
                    args.vvix_threshold
                );
            }
            if args.vvix_window < 2 {
                bail!(
                    "vvix_regime requires --vvix-window >= 2, got {}",
                    args.vvix_window
                );
            }
            if args.vvix_mode != "risk_off" && args.vvix_mode != "contrarian" {
                bail!(
                    "vvix_regime requires --vvix-mode risk_off|contrarian, got '{}'",
                    args.vvix_mode
                );
            }
        }
        _ => {}
    }

    if !quiet {
        println!("Loading {asset} from bronze parquet...");
    }
    let mut bars = load_ticker_ohlcv(&asset, None, None)?;
    let requested_start_date = args
        .start_date
        .as_deref()
        .map(|value| {
            value
                .parse::<NaiveDate>()
                .map_err(|e| anyhow::anyhow!("invalid --start-date: {e}"))
        })
        .transpose()?;
    let end_date = args
        .end_date
        .parse::<NaiveDate>()
        .map_err(|e| anyhow::anyhow!("invalid --end-date: {e}"))?;
    if let Some(start_date) = requested_start_date {
        if start_date > end_date {
            bail!("--start-date must be on or before --end-date");
        }
    }
    let max_lookback = args
        .slow_window
        .max(args.vol_window)
        .max(args.rsi_window.max(2))
        .max(args.vvix_window);
    let load_start_date = if let Some(start_date) = requested_start_date {
        start_date - chrono::Duration::days(indicator_buffer_days(max_lookback))
    } else {
        let buffer = args
            .sessions
            .saturating_mul(6)
            .max(max_lookback.saturating_mul(3))
            .saturating_add(30) as i64;
        end_date - chrono::Duration::days(buffer)
    };

    bars.retain(|r| r.trade_date >= load_start_date && r.trade_date <= end_date);
    let min_required_rows = if requested_start_date.is_some() {
        max_lookback.max(2)
    } else {
        args.sessions.max(40)
    };
    if bars.len() < min_required_rows {
        bail!(
            "insufficient data from {load_start_date} to {end_date} for {asset}; available {} rows",
            bars.len()
        );
    }
    bars.sort_by_key(|r| r.trade_date);

    let dates: Vec<NaiveDate> = bars.iter().map(|r| r.trade_date).collect();
    let closes: Vec<f64> = bars.iter().map(|r| r.close).collect();
    if closes.len() < 2 {
        bail!("not enough observations after filtering for {asset}");
    }

    // Load VIX data when needed (vol_spread rule)
    let vix_aligned: Option<Vec<f64>> = if matches!(rule, ResearchRule::VolSpread) {
        let vix_bars = load_vix_ohlcv(None)?;
        let vix_map: HashMap<NaiveDate, f64> =
            vix_bars.iter().map(|r| (r.trade_date, r.close)).collect();
        Some(
            dates
                .iter()
                .map(|d| *vix_map.get(d).unwrap_or(&f64::NAN))
                .collect(),
        )
    } else {
        None
    };

    // Load VVIX data when needed (vvix_regime rule)
    let vvix_aligned: Option<Vec<f64>> = if matches!(rule, ResearchRule::VvixRegime) {
        let vvix_bars = load_volatility_index_ohlcv("VVIX", None)?;
        let vvix_map: HashMap<NaiveDate, f64> =
            vvix_bars.iter().map(|r| (r.trade_date, r.close)).collect();
        Some(
            dates
                .iter()
                .map(|d| *vvix_map.get(d).unwrap_or(&f64::NAN))
                .collect(),
        )
    } else {
        None
    };

    let masks = build_signal_mask(
        &closes,
        rule,
        args.fast_window,
        args.slow_window,
        args.rsi_window,
        args.rsi_oversold,
        args.vol_window,
        args.vol_cap,
        vix_aligned.as_deref(),
        args.mr_entry_threshold,
        vvix_aligned.as_deref(),
        args.vvix_window,
        args.vvix_threshold,
        args.vvix_mode == "contrarian",
    );
    let start_idx = evaluation_start_index(&dates, requested_start_date, args.sessions)?;
    let eval_dates = &dates[start_idx..];
    let eval_closes = &closes[start_idx..];
    let eval_mask = &masks[start_idx..];
    if eval_closes.len() < 2 {
        bail!(
            "not enough observations in evaluation window for {asset}; need at least 2 bars, got {}",
            eval_closes.len()
        );
    }

    let mut strategies = Vec::new();
    strategies.push(StrategyResult {
        name: "Buy & Hold".to_string(),
        equity: buy_and_hold_equity(eval_closes, DEFAULT_CAPITAL),
    });

    let eq_signal = simulate_strategy(eval_closes, eval_mask, DEFAULT_CAPITAL);
    strategies.push(StrategyResult {
        name: format!("PaperResearch [{}|{}]", asset, rule.as_str()),
        equity: eq_signal,
    });

    let years = (eval_dates
        .last()
        .ok_or_else(|| anyhow::anyhow!("missing data range"))?
        .signed_duration_since(*eval_dates.first().unwrap()))
    .num_days() as f64
        / DEFAULT_TTLYRS;

    if json_mode {
        let results: Vec<_> = strategies
            .iter()
            .map(|s| compute_strategy_metrics(&s.name, &s.equity, years))
            .collect();
        let audit = if args.include_audit {
            Some(serde_json::to_value(build_execution_audit(
                &asset,
                rule,
                eval_dates,
                eval_closes,
                eval_mask,
                DEFAULT_CAPITAL,
                requested_start_date,
                end_date,
            ))?)
        } else {
            None
        };
        let output = JsonOutput {
            strategy: "paper-research".to_string(),
            ticker: asset.clone(),
            period_start: eval_dates.first().unwrap().to_string(),
            period_end: eval_dates.last().unwrap().to_string(),
            years,
            capital: DEFAULT_CAPITAL,
            fee_model: "IBKR Tiered".to_string(),
            results,
            annual_returns: build_json_annual_returns(
                &strategies,
                eval_dates,
                args.start_year_table,
            ),
            audit,
        };
        println!("{}", serde_json::to_string(&output)?);
    } else if fmt == OutputFormat::Md {
        println!(
            "{}",
            crate::strategies::common::format_results_md(
                &format!("Paper Research: {} {}", asset, rule.as_str()),
                &asset,
                eval_dates,
                years,
                DEFAULT_CAPITAL,
                &strategies,
                &[],
                args.start_year_table,
            )
        );
    } else {
        println!();
        println!("{}", "=".repeat(80));
        println!("Paper Research Strategy: {} ({})", asset, rule.as_str());
        println!(
            "Period: {} to {} ({:.1} years)",
            eval_dates.first().unwrap(),
            eval_dates.last().unwrap(),
            years
        );
        println!(
            "Capital: ${}",
            (DEFAULT_CAPITAL as i64).to_formatted_string(&num_format::Locale::en)
        );
        println!("{}", "=".repeat(80));

        let (header, sep) = format_results_header();
        println!("{header}");
        println!("{sep}");
        for strategy in &strategies {
            println!(
                "{}",
                format_strategy_row(&strategy.name, &strategy.equity, years)
            );
        }
        println!("\nAnnual Returns (from {}):", args.start_year_table);
        println!(
            "{}",
            format_annual_table(&strategies, eval_dates, args.start_year_table, 25)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moving_average_window() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        assert!((moving_average(&values, 1, 2).unwrap() - 1.5).abs() < 1e-12);
        assert!((moving_average(&values, 3, 3).unwrap() - 3.0).abs() < 1e-12);
    }

    #[test]
    fn test_rsi_bounds() {
        let closes = vec![100.0, 102.0, 101.0, 104.0, 103.0, 105.0];
        let rsi = rolling_rsi(&closes, 4, 4).unwrap_or(-1.0);
        assert!((0.0..=100.0).contains(&rsi));
    }

    #[test]
    fn test_signal_mask_modes() {
        let closes = vec![10.0, 10.5, 10.8, 10.2, 10.4, 10.9, 11.0, 11.2];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::TrendMomentum,
            3,
            5,
            3,
            30.0,
            3,
            0.5,
            None,
            0.02,
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), closes.len());
    }

    #[test]
    fn test_vol_spread_signal_positive_threshold() {
        // VRP harvest: VIX >> realized vol → signal should fire
        // Build closes with low realized vol (~0 variation)
        let n = 50;
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + (i as f64) * 0.01).collect();
        // VIX at 30 (implied = 0.30) while realized will be very low
        let vix: Vec<f64> = vec![30.0; n];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::VolSpread,
            12,
            40,
            14,
            30.0,
            20,   // vol_window
            0.20, // positive threshold → VRP harvest
            Some(&vix),
            0.02,
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), n);
        // After warmup, signals should fire because VIX >> realized
        let active = mask.iter().filter(|&&b| b).count();
        assert!(
            active > 0,
            "expected VRP harvest signals to fire with high VIX and low realized vol"
        );
    }

    #[test]
    fn test_vol_spread_no_vix_data() {
        // No VIX data → all signals false
        let n = 50;
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + (i as f64) * 0.5).collect();
        let mask = build_signal_mask(
            &closes,
            ResearchRule::VolSpread,
            12,
            40,
            14,
            30.0,
            20,
            0.20,
            None, // no VIX
            0.02,
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), n);
        assert!(
            mask.iter().all(|&b| !b),
            "expected all-false with no VIX data"
        );
    }

    #[test]
    fn test_vol_spread_negative_threshold_snapback() {
        // Snap-back: realized >> implied → signal fires when spread < vol_cap (negative)
        let n = 60;
        // Create high-volatility closes (big swings)
        let mut closes = Vec::with_capacity(n);
        for i in 0..n {
            closes.push(100.0 + if i % 2 == 0 { 10.0 } else { -10.0 } * (i as f64 / n as f64));
        }
        // VIX at 10 (implied = 0.10), very low relative to the swings
        let vix: Vec<f64> = vec![10.0; n];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::VolSpread,
            12,
            40,
            14,
            30.0,
            20,
            -0.10, // negative threshold → snap-back
            Some(&vix),
            0.02,
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), n);
        // With high realized vol and low VIX, spread is very negative → should trigger
        let active = mask.iter().filter(|&&b| b).count();
        assert!(
            active > 0,
            "expected snap-back signals to fire with high realized vol and low VIX"
        );
    }

    #[test]
    fn test_evaluation_start_index_prefers_requested_start_date() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap(),
            NaiveDate::from_ymd_opt(2024, 12, 31).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(),
        ];

        let start_idx = evaluation_start_index(
            &dates,
            Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            252,
        )
        .expect("requested start should resolve to first trading bar");

        assert_eq!(start_idx, 2);
    }

    #[test]
    fn test_build_execution_audit_reconstructs_equity_path() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 7).unwrap(),
        ];
        let closes = vec![100.0, 105.0, 103.0, 106.0];
        let mask = vec![true, false, true, false];

        let equity = simulate_strategy(&closes, &mask, 1_000_000.0);
        let audit = build_execution_audit(
            "QQQ",
            ResearchRule::TrendMomentum,
            &dates,
            &closes,
            &mask,
            1_000_000.0,
            Some(NaiveDate::from_ymd_opt(2025, 1, 2).unwrap()),
            NaiveDate::from_ymd_opt(2025, 1, 7).unwrap(),
        );

        assert_eq!(audit.executed_trade_count, 2);
        assert_eq!(audit.trades.len(), 2);
        assert_eq!(audit.actual_period_start, "2025-01-02");
        assert_eq!(audit.actual_period_end, "2025-01-07");
        assert_eq!(audit.equity_trace.len(), dates.len());
        assert!(
            (audit.ending_equity - *equity.last().unwrap()).abs() < 1e-9,
            "audit ending equity should match simulated ending equity"
        );
    }

    #[test]
    fn test_mean_reversion_filter_triggers_below_fair_value() {
        // Price dips 3% below its 5-bar SMA → signal should fire
        let closes = vec![100.0, 101.0, 102.0, 103.0, 104.0, 97.0];
        // SMA(5) at idx=5 = (101+102+103+104+97)/5 = 101.4
        // delta = (97 - 101.4) / 101.4 ≈ -0.0434 < -0.02 → true
        let mask = build_signal_mask(
            &closes,
            ResearchRule::MeanReversionFilter,
            12,
            5, // slow_window = SMA period
            14,
            30.0,
            20,
            0.45,
            None,
            0.02, // entry threshold
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), closes.len());
        assert!(mask[5], "expected signal when price is 3%+ below fair value SMA");
    }

    #[test]
    fn test_mean_reversion_filter_no_signal_above_fair_value() {
        // Steadily rising prices — never below SMA
        let closes = vec![100.0, 102.0, 104.0, 106.0, 108.0, 110.0, 112.0, 114.0];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::MeanReversionFilter,
            12,
            5,
            14,
            30.0,
            20,
            0.45,
            None,
            0.02,
            None, 63, 0.75, false,
        );
        assert_eq!(mask.len(), closes.len());
        assert!(
            mask.iter().all(|&b| !b),
            "expected no signals for steadily rising prices"
        );
    }

    #[test]
    fn test_mean_reversion_filter_threshold_sensitivity() {
        // Same series: tight threshold triggers, wide threshold doesn't
        let closes = vec![100.0, 101.0, 102.0, 103.0, 104.0, 102.5];
        // SMA(5) at idx=5 = (101+102+103+104+102.5)/5 = 102.5
        // delta = (102.5 - 102.5) / 102.5 = 0.0 → neither fires

        // Use a price that's about 1.5% below SMA
        let closes2 = vec![100.0, 101.0, 102.0, 103.0, 104.0, 100.5];
        // SMA(5) at idx=5 = (101+102+103+104+100.5)/5 = 102.1
        // delta = (100.5 - 102.1) / 102.1 ≈ -0.0157

        let mask_tight = build_signal_mask(
            &closes2,
            ResearchRule::MeanReversionFilter,
            12, 5, 14, 30.0, 20, 0.45, None,
            0.01, // tight threshold: 1% → fires
            None, 63, 0.75, false,
        );
        let mask_wide = build_signal_mask(
            &closes2,
            ResearchRule::MeanReversionFilter,
            12, 5, 14, 30.0, 20, 0.45, None,
            0.05, // wide threshold: 5% → doesn't fire
            None, 63, 0.75, false,
        );
        let tight_count = mask_tight.iter().filter(|&&b| b).count();
        let wide_count = mask_wide.iter().filter(|&&b| b).count();
        assert!(
            tight_count > wide_count,
            "tighter threshold should produce more signals: tight={tight_count} wide={wide_count}"
        );
    }

    #[test]
    fn test_mean_reversion_filter_insufficient_lookback() {
        // Fewer bars than slow_window → all false
        let closes = vec![100.0, 101.0, 102.0];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::MeanReversionFilter,
            12,
            10, // slow_window=10 but only 3 bars
            14,
            30.0,
            20,
            0.45,
            None,
            0.02,
            None, 63, 0.75, false,
        );
        assert!(mask.iter().all(|&b| !b), "expected all-false with insufficient data");
    }

    #[test]
    fn test_vvix_regime_risk_off_long_when_low() {
        // VVIX low → percentile rank low → risk_off mode should go long
        let n = 80;
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + (i as f64) * 0.1).collect();
        // VVIX: mostly high (90-110), but drops to 70 at the end
        let mut vvix: Vec<f64> = (0..n).map(|i| 90.0 + (i % 10) as f64 * 2.0).collect();
        vvix[n - 1] = 70.0; // very low VVIX

        let mask = build_signal_mask(
            &closes,
            ResearchRule::VvixRegime,
            12, 40, 14, 30.0, 20, 0.45,
            None, 0.02,
            Some(&vvix),
            20, // vvix_window
            0.75, // threshold
            false, // risk_off mode
        );
        assert_eq!(mask.len(), n);
        assert!(mask[n - 1], "expected long signal when VVIX is at low percentile in risk_off mode");
    }

    #[test]
    fn test_vvix_regime_contrarian_long_when_high() {
        // VVIX high → percentile rank high → contrarian mode should go long
        let n = 80;
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + (i as f64) * 0.1).collect();
        // VVIX: mostly low (80-90), spikes to 130 at the end
        let mut vvix: Vec<f64> = (0..n).map(|_| 85.0).collect();
        vvix[n - 1] = 130.0; // very high VVIX

        let mask = build_signal_mask(
            &closes,
            ResearchRule::VvixRegime,
            12, 40, 14, 30.0, 20, 0.45,
            None, 0.02,
            Some(&vvix),
            20,
            0.75,
            true, // contrarian mode
        );
        assert_eq!(mask.len(), n);
        assert!(mask[n - 1], "expected long signal when VVIX is at high percentile in contrarian mode");
    }

    #[test]
    fn test_vvix_regime_no_signal_without_data() {
        let n = 80;
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + (i as f64) * 0.1).collect();
        let mask = build_signal_mask(
            &closes,
            ResearchRule::VvixRegime,
            12, 40, 14, 30.0, 20, 0.45,
            None, 0.02,
            None, // no VVIX data
            20, 0.75, false,
        );
        assert!(mask.iter().all(|&b| !b), "expected all-false with no VVIX data");
    }

    #[test]
    fn test_vvix_regime_insufficient_lookback() {
        let n = 5;
        let closes: Vec<f64> = vec![100.0; n];
        let vvix: Vec<f64> = vec![90.0; n];
        let mask = build_signal_mask(
            &closes,
            ResearchRule::VvixRegime,
            12, 40, 14, 30.0, 20, 0.45,
            None, 0.02,
            Some(&vvix),
            20, // vvix_window=20 but only 5 bars
            0.75, false,
        );
        assert!(mask.iter().all(|&b| !b), "expected all-false with insufficient VVIX data");
    }
}
