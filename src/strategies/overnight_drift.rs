/// Overnight Drift Backtesting Engine.
///
/// Buy SPY at close, sell at next open.
/// Optional VIX regime filter: only take overnight trades when VIX < 200-day MA.
use anyhow::Result;
use chrono::NaiveDate;

use num_format::ToFormattedString;

use crate::cli::OutputFormat;
use crate::data::readers::{OhlcvRow, VixRow, load_ticker_ohlcv, load_vix_from_cboe};
use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::metrics::performance::sharpe_default;
use crate::strategies::common::{
    JsonOutput, StrategyResult, build_json_annual_returns, buy_and_hold_equity,
    compute_strategy_metrics, daily_returns, format_annual_table, format_results_header,
    format_strategy_row,
};

pub const DEFAULT_CAPITAL: f64 = 1_000_000.0;

/// Compute overnight log returns: ln(Open_{t+1} / Close_t).
pub fn compute_overnight_returns(rows: &[OhlcvRow]) -> Vec<f64> {
    let n = rows.len();
    let mut returns = Vec::with_capacity(n);
    for i in 0..n {
        if i + 1 < n {
            returns.push((rows[i + 1].open / rows[i].close).ln());
        } else {
            returns.push(f64::NAN);
        }
    }
    returns
}

/// Compute VIX filter: True when VIX close < VIX MA (low-vol regime).
pub fn compute_vix_filter(
    vix_rows: &[VixRow],
    lookback: usize,
) -> Vec<(NaiveDate, f64, f64, bool)> {
    let n = vix_rows.len();
    let mut result = Vec::with_capacity(n);

    for i in 0..n {
        let vix_close = vix_rows[i].close;
        let vix_ma = if i + 1 >= lookback {
            let sum: f64 = vix_rows[i + 1 - lookback..=i].iter().map(|r| r.close).sum();
            sum / lookback as f64
        } else {
            f64::NAN
        };

        let filter = if vix_ma.is_finite() {
            vix_close < vix_ma
        } else {
            false
        };

        result.push((vix_rows[i].trade_date, vix_close, vix_ma, filter));
    }

    result
}

/// Simulate overnight strategy with equity-tracking loop.
pub fn simulate_strategy(
    returns: &[f64],
    closes: &[f64],
    opens_next: &[f64],
    mask: &[bool],
    capital: f64,
    fee_fn: &dyn Fn(f64, f64) -> f64,
) -> Vec<f64> {
    let n = returns.len();
    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    let mut current = capital;

    for i in 0..n {
        if mask[i] && returns[i].is_finite() {
            let shares = (current / closes[i]) as i64;
            if shares > 0 {
                let cost = fee_fn(current, closes[i]);
                let pnl = shares as f64 * (opens_next[i] - closes[i]);
                current = current + pnl - cost;
            }
        }
        equity.push(current);
    }

    equity
}

/// Augmented Dickey-Fuller test on returns series (pure Rust via nalgebra).
///
/// Returns (adf_statistic, p_value).
pub fn adf_test(returns: &[f64]) -> (f64, f64) {
    let clean: Vec<f64> = returns.iter().copied().filter(|x| x.is_finite()).collect();
    if clean.len() < 20 {
        return (0.0, 1.0);
    }

    let max_lag = 10usize;

    // Compute first differences
    let diff: Vec<f64> = clean.windows(2).map(|w| w[1] - w[0]).collect();

    // Try different lag lengths, pick by AIC
    let mut best_aic = f64::INFINITY;
    let mut best_stat = 0.0;

    for lag in 0..=max_lag.min(diff.len().saturating_sub(5)) {
        let start = lag + 1;
        if start >= diff.len() {
            break;
        }
        let y: Vec<f64> = diff[start..].to_vec();
        let obs = y.len();
        if obs < lag + 3 {
            break;
        }

        // Build regressor matrix: [y_lag1, diff_lag1, ..., diff_lagp, 1]
        let ncols = 1 + lag + 1; // y_lag1 + p lagged diffs + intercept
        let mut x_data = Vec::with_capacity(obs * ncols);

        for i in 0..obs {
            let t = start + i;
            // y_{t-1} (level)
            x_data.push(clean[t]);
            // lagged diffs
            for j in 1..=lag {
                x_data.push(diff[t - j]);
            }
            // intercept
            x_data.push(1.0);
        }

        let x = nalgebra::DMatrix::from_row_slice(obs, ncols, &x_data);
        let y_vec = nalgebra::DVector::from_column_slice(&y);

        // OLS: beta = (X'X)^{-1} X'y
        let xtx = x.transpose() * &x;
        let xty = x.transpose() * &y_vec;

        let decomp = xtx.clone().lu();
        let beta = match decomp.solve(&xty) {
            Some(b) => b,
            None => continue,
        };

        let residuals = &y_vec - &x * &beta;
        let sse: f64 = residuals.iter().map(|r| r * r).sum();
        let sigma2 = sse / (obs - ncols) as f64;

        // AIC = n * ln(sse/n) + 2 * k
        let aic = obs as f64 * (sse / obs as f64).ln() + 2.0 * ncols as f64;

        if aic < best_aic {
            best_aic = aic;
            // ADF statistic = beta[0] / se(beta[0])
            let xtx_inv = match xtx.try_inverse() {
                Some(inv) => inv,
                None => continue,
            };
            let se_beta0 = (sigma2 * xtx_inv[(0, 0)]).sqrt();
            if se_beta0 > 0.0 {
                best_stat = beta[0] / se_beta0;
            }
        }
    }

    // Approximate p-value using MacKinnon critical values (constant only)
    // Critical values for N=infinity: 1%=-3.43, 5%=-2.86, 10%=-2.57
    let p_value = if best_stat < -3.43 {
        0.005
    } else if best_stat < -2.86 {
        0.03
    } else if best_stat < -2.57 {
        0.07
    } else if best_stat < -1.94 {
        0.30
    } else {
        0.80
    };

    (best_stat, p_value)
}

/// CLI arguments for the overnight-drift strategy.
#[derive(Debug, clap::Args)]
pub struct OvernightDriftArgs {
    #[arg(long, help = "Start date (YYYY-MM-DD)")]
    pub start_date: Option<String>,

    #[arg(long, help = "End date (YYYY-MM-DD)")]
    pub end_date: Option<String>,

    #[arg(long, default_value_t = DEFAULT_CAPITAL, help = "Starting capital")]
    pub capital: f64,

    #[arg(long, help = "Skip VIX-filtered strategy")]
    pub no_vix_filter: bool,

    #[arg(long, help = "Skip chart generation")]
    pub no_plots: bool,

    #[arg(long, default_value_t = 2015, help = "Annual table start year")]
    pub start_year_table: i32,
}

/// Run the overnight drift strategy.
pub fn run(args: &OvernightDriftArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let quiet = json_mode || fmt == OutputFormat::Md;
    if !quiet {
        println!("Loading SPY from bronze parquet...");
    }
    let mut spy = load_ticker_ohlcv("SPY", None, None)?;
    if !quiet {
        println!(
            "  SPY: {} bars, {} to {}",
            spy.len(),
            spy.first().unwrap().trade_date,
            spy.last().unwrap().trade_date
        );
    }

    let include_vix = !args.no_vix_filter;
    let vix_filter_data = if include_vix {
        if !quiet {
            println!("Loading VIX from CBOE...");
        }
        let vix_raw = load_vix_from_cboe(None)?;
        if !quiet {
            println!(
                "  VIX: {} bars, {} to {}",
                vix_raw.len(),
                vix_raw.first().unwrap().trade_date,
                vix_raw.last().unwrap().trade_date
            );
        }
        let vix = compute_vix_filter(&vix_raw, 200);
        Some(vix)
    } else {
        None
    };

    // Date filtering
    if let Some(ref sd) = args.start_date {
        let start = sd.parse::<NaiveDate>()?;
        spy.retain(|r| r.trade_date >= start);
    }
    if let Some(ref ed) = args.end_date {
        let end = ed.parse::<NaiveDate>()?;
        spy.retain(|r| r.trade_date <= end);
    }

    let overnight_full = compute_overnight_returns(&spy);
    let n_full = spy.len();

    // Build opens_next array
    let opens_next: Vec<f64> = (0..n_full)
        .map(|i| {
            if i + 1 < n_full {
                spy[i + 1].open
            } else {
                f64::NAN
            }
        })
        .collect();

    // Trim last row
    let n = n_full - 1;
    let dates: Vec<NaiveDate> = spy[..n].iter().map(|r| r.trade_date).collect();
    let closes: Vec<f64> = spy[..n].iter().map(|r| r.close).collect();
    let opens_next: Vec<f64> = opens_next[..n].to_vec();
    let overnight: Vec<f64> = overnight_full[..n].to_vec();

    // Build VIX filter mask
    let vix_mask: Vec<bool> = if let Some(ref vfd) = vix_filter_data {
        let vix_map: std::collections::HashMap<NaiveDate, bool> =
            vfd.iter().map(|(d, _, _, f)| (*d, *f)).collect();
        dates
            .iter()
            .map(|d| *vix_map.get(d).unwrap_or(&false))
            .collect()
    } else {
        vec![false; n]
    };

    let mut strategies: Vec<StrategyResult> = Vec::new();

    // Buy & Hold
    let bh_equity = buy_and_hold_equity(&closes, args.capital);
    strategies.push(StrategyResult {
        name: "Buy & Hold".to_string(),
        equity: bh_equity,
    });

    // Overnight (All)
    let mask_all = vec![true; n];
    let eq_all = simulate_strategy(
        &overnight,
        &closes,
        &opens_next,
        &mask_all,
        args.capital,
        &ibkr_roundtrip_cost,
    );
    strategies.push(StrategyResult {
        name: "Overnight (All)".to_string(),
        equity: eq_all,
    });

    // Overnight (VIX Filter)
    if include_vix {
        let eq_vix = simulate_strategy(
            &overnight,
            &closes,
            &opens_next,
            &vix_mask,
            args.capital,
            &ibkr_roundtrip_cost,
        );
        strategies.push(StrategyResult {
            name: "Overnight (VIX Filter)".to_string(),
            equity: eq_vix,
        });
    }

    let years = (dates
        .last()
        .unwrap()
        .signed_duration_since(*dates.first().unwrap()))
    .num_days() as f64
        / 365.25;

    // Compute ADF stats for overnight strategies
    let mut adf_results: Vec<(String, f64, f64)> = Vec::new();
    for strat in &strategies {
        if strat.name.contains("Overnight") {
            let dr = daily_returns(&strat.equity);
            let (adf_stat, p_val) = adf_test(&dr);
            adf_results.push((strat.name.clone(), adf_stat, p_val));
        }
    }

    if json_mode {
        let mut results: Vec<_> = strategies
            .iter()
            .map(|s| compute_strategy_metrics(&s.name, &s.equity, years))
            .collect();

        // Attach ADF stats
        for m in &mut results {
            if let Some((_, stat, pval)) = adf_results.iter().find(|(n, _, _)| *n == m.name) {
                m.adf_stat = Some(*stat);
                m.adf_p_value = Some(*pval);
            }
        }

        let output = JsonOutput {
            strategy: "overnight-drift".to_string(),
            ticker: "SPY".to_string(),
            period_start: dates.first().unwrap().to_string(),
            period_end: dates.last().unwrap().to_string(),
            years,
            capital: args.capital,
            fee_model: "IBKR Tiered".to_string(),
            results,
            annual_returns: build_json_annual_returns(&strategies, &dates, args.start_year_table),
            audit: None,
        };
        println!("{}", serde_json::to_string(&output)?);
    } else if fmt == OutputFormat::Md {
        println!(
            "{}",
            crate::strategies::common::format_results_md(
                "Overnight Drift Backtest",
                "SPY",
                &dates,
                years,
                args.capital,
                &strategies,
                &adf_results,
                args.start_year_table,
            )
        );
    } else {
        println!();
        println!("{}", "=".repeat(80));
        println!("OVERNIGHT DRIFT BACKTEST RESULTS");
        println!(
            "Period: {} to {} ({:.1} years)",
            dates.first().unwrap(),
            dates.last().unwrap(),
            years
        );
        println!(
            "Capital: ${} | Fee model: IBKR Tiered",
            (args.capital as i64).to_formatted_string(&num_format::Locale::en)
        );
        println!("Note: adj_close == close (IB split-adj only); B&H CAGR understates by ~1.3%/yr");
        println!("{}", "=".repeat(80));

        let (header, sep) = format_results_header();
        println!("{header}");
        println!("{sep}");

        for strat in &strategies {
            println!("{}", format_strategy_row(&strat.name, &strat.equity, years));
            if let Some((_, stat, pval)) = adf_results.iter().find(|(n, _, _)| *n == strat.name) {
                println!("  {:>23}: {:.4}  p-value: {:.6}", "ADF stat", stat, pval);
            }
        }

        println!("\nAnnual Returns (from {}):", args.start_year_table);
        println!(
            "{}",
            format_annual_table(&strategies, &dates, args.start_year_table, 20)
        );

        if !args.no_plots {
            println!("\nPlotting not yet implemented in Rust port (use --no-plots)");
        }

        if include_vix {
            if let Some(vix_strat) = strategies.iter().find(|s| s.name.contains("VIX Filter")) {
                let all_strat = strategies
                    .iter()
                    .find(|s| s.name == "Overnight (All)")
                    .unwrap();
                let dr_all = daily_returns(&all_strat.equity);
                let dr_vix = daily_returns(&vix_strat.equity);
                let s_all = sharpe_default(&dr_all);
                let s_vix = sharpe_default(&dr_vix);
                let vix_days: usize = vix_mask.iter().filter(|&&v| v).count();
                let total_days = n;
                println!(
                    "\nVIX Filter traded {}/{} days ({:.0}%)",
                    vix_days,
                    total_days,
                    vix_days as f64 / total_days as f64 * 100.0
                );
                if s_vix > s_all {
                    println!(
                        "VIX filter improved Sharpe: {:.2} -> {:.2} by avoiding high-vol gap risk",
                        s_all, s_vix
                    );
                } else {
                    println!(
                        "VIX filter Sharpe: {:.2} vs unfiltered: {:.2}",
                        s_vix, s_all
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::readers::OhlcvRow;

    fn make_rows(opens: &[f64], closes: &[f64]) -> Vec<OhlcvRow> {
        opens
            .iter()
            .zip(closes.iter())
            .enumerate()
            .map(|(i, (&o, &c))| OhlcvRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i as u32).unwrap(),
                open: o,
                high: o.max(c) + 1.0,
                low: o.min(c) - 1.0,
                close: c,
                volume: 1_000_000.0,
            })
            .collect()
    }

    #[test]
    fn test_compute_overnight_returns() {
        let rows = make_rows(&[100.0, 102.0, 104.0], &[101.0, 103.0, 105.0]);
        let ret = compute_overnight_returns(&rows);
        let expected_0 = (102.0_f64 / 101.0).ln();
        let expected_1 = (104.0_f64 / 103.0).ln();
        assert!((ret[0] - expected_0).abs() < 1e-10);
        assert!((ret[1] - expected_1).abs() < 1e-10);
        assert!(ret[2].is_nan());
    }

    #[test]
    fn test_simulate_basic() {
        let closes = [100.0, 101.0, 102.0];
        let opens_next = [101.5, 102.5, 103.5];
        let returns: Vec<f64> = closes
            .iter()
            .zip(opens_next.iter())
            .map(|(c, o): (&f64, &f64)| (o / c).ln())
            .collect();
        let mask = [true, true, true];

        let equity =
            simulate_strategy(&returns, &closes, &opens_next, &mask, 10_000.0, &|_, _| 0.0);
        assert_eq!(equity.len(), 4);
        assert_eq!(equity[0], 10_000.0);
        assert!((equity[1] - 10_150.0).abs() < 1e-6);
    }

    #[test]
    fn test_mask_skips_trades() {
        let closes = [100.0, 100.0];
        let opens_next = [110.0, 110.0];
        let returns: Vec<f64> = closes
            .iter()
            .zip(opens_next.iter())
            .map(|(c, o): (&f64, &f64)| (o / c).ln())
            .collect();
        let mask = [false, true];

        let equity =
            simulate_strategy(&returns, &closes, &opens_next, &mask, 10_000.0, &|_, _| 0.0);
        assert_eq!(equity[1], 10_000.0);
        assert!(equity[2] > 10_000.0);
    }

    #[test]
    fn test_fees_reduce_equity() {
        let closes = [100.0];
        let opens_next = [100.0];
        let returns = [(100.0_f64 / 100.0).ln()];
        let mask = [true];

        let equity = simulate_strategy(
            &returns,
            &closes,
            &opens_next,
            &mask,
            10_000.0,
            &ibkr_roundtrip_cost,
        );
        assert!(equity[1] < 10_000.0);
    }

    #[test]
    fn test_vix_filter_ma_crossover() {
        let mut vix_rows = Vec::new();
        // 200 rows at 30.0, then 10 at 15.0
        for i in 0..210 {
            let close = if i < 200 { 30.0 } else { 15.0 };
            vix_rows.push(VixRow {
                trade_date: NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
                    + chrono::Duration::days(i as i64),
                open: close,
                high: close,
                low: close,
                close,
            });
        }
        let result = compute_vix_filter(&vix_rows, 200);
        // First 199 should have NaN MA or not trigger
        assert!(!result[198].3);
        assert!(!result[199].3);
        // Last should have vix_close=15 < MA~30 -> true
        assert!(result[209].3);
    }

    #[test]
    fn test_adf_stationary_data() {
        // Random stationary data should reject null (small p-value)
        let mut rng_state: u64 = 42;
        let mut returns = Vec::with_capacity(500);
        for _ in 0..500 {
            // Simple LCG pseudo-random
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let u = (rng_state >> 33) as f64 / (1u64 << 31) as f64;
            returns.push((u - 0.5) * 0.02);
        }
        let (stat, pval) = adf_test(&returns);
        assert!(
            stat < -2.0,
            "ADF stat should be negative for stationary data: {stat}"
        );
        assert!(pval < 0.5, "p-value should be small: {pval}");
    }

    #[test]
    fn test_single_row_returns_nan() {
        let rows = make_rows(&[100.0], &[101.0]);
        let ret = compute_overnight_returns(&rows);
        assert!(ret[0].is_nan());
    }

    #[test]
    fn test_vix_boundary_equal() {
        // When VIX close equals MA, filter should be false (strict <)
        let mut vix_rows = Vec::new();
        for i in 0..200 {
            vix_rows.push(VixRow {
                trade_date: NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
                    + chrono::Duration::days(i as i64),
                open: 20.0,
                high: 20.0,
                low: 20.0,
                close: 20.0,
            });
        }
        let result = compute_vix_filter(&vix_rows, 200);
        // Last row: close=20, MA=20, should be false (not strictly less than)
        assert!(!result[199].3);
    }
}
