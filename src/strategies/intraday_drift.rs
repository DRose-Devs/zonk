/// Intraday Drift Backtesting Engine.
///
/// Buy at the open, sell at the close same day. Optional short mode.
use anyhow::Result;
use chrono::NaiveDate;

use num_format::ToFormattedString;

use crate::cli::OutputFormat;
use crate::data::readers::{OhlcvRow, load_ticker_ohlcv};
use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::strategies::common::{
    JsonOutput, StrategyResult, build_json_annual_returns, buy_and_hold_equity,
    compute_strategy_metrics, format_annual_table, format_results_header, format_strategy_row,
};

pub const DEFAULT_CAPITAL: f64 = 1_000_000.0;

/// Compute intraday log returns: ln(Close_t / Open_t).
pub fn compute_intraday_returns(rows: &[OhlcvRow]) -> Vec<f64> {
    rows.iter().map(|r| (r.close / r.open).ln()).collect()
}

/// Simulate intraday strategy with equity-tracking loop.
///
/// Long: buy at open, sell at close same day.
/// Short: sell at open, cover at close same day.
pub fn simulate_strategy(
    opens: &[f64],
    closes: &[f64],
    mask: &[bool],
    capital: f64,
    fee_fn: &dyn Fn(f64, f64) -> f64,
    short: bool,
) -> Vec<f64> {
    let n = opens.len();
    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    let mut current = capital;
    let direction: f64 = if short { -1.0 } else { 1.0 };

    for i in 0..n {
        if mask[i] {
            let shares = (current / opens[i]) as i64;
            if shares > 0 {
                let cost = fee_fn(current, opens[i]);
                let pnl = direction * shares as f64 * (closes[i] - opens[i]);
                current = current + pnl - cost;
            }
        }
        equity.push(current);
    }

    equity
}

/// CLI arguments for the intraday-drift strategy.
#[derive(Debug, clap::Args)]
pub struct IntradayDriftArgs {
    #[arg(long, help = "Start date (YYYY-MM-DD)")]
    pub start_date: Option<String>,

    #[arg(long, help = "End date (YYYY-MM-DD)")]
    pub end_date: Option<String>,

    #[arg(long, default_value_t = DEFAULT_CAPITAL, help = "Starting capital")]
    pub capital: f64,

    #[arg(long, help = "Skip chart generation")]
    pub no_plots: bool,

    #[arg(long, default_value = "SPY", help = "Ticker symbol")]
    pub ticker: String,

    #[arg(long, help = "Short at open, cover at close")]
    pub short: bool,

    #[arg(long, default_value_t = 2015, help = "Annual table start year")]
    pub start_year_table: i32,
}

/// Run the intraday drift strategy.
pub fn run(args: &IntradayDriftArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let quiet = json_mode || fmt == OutputFormat::Md;
    let ticker = args.ticker.to_uppercase();

    if !quiet {
        println!("Loading {} from bronze parquet...", ticker);
    }
    let mut spy = load_ticker_ohlcv(&ticker, None, None)?;
    if !quiet {
        println!(
            "  {}: {} bars, {} to {}",
            ticker,
            spy.len(),
            spy.first().unwrap().trade_date,
            spy.last().unwrap().trade_date
        );
    }

    // Date filtering
    if let Some(ref sd) = args.start_date {
        let start = sd.parse::<NaiveDate>()?;
        spy.retain(|r| r.trade_date >= start);
    }
    if let Some(ref ed) = args.end_date {
        let end = ed.parse::<NaiveDate>()?;
        spy.retain(|r| r.trade_date <= end);
    }

    let n = spy.len();
    let dates: Vec<NaiveDate> = spy.iter().map(|r| r.trade_date).collect();
    let opens: Vec<f64> = spy.iter().map(|r| r.open).collect();
    let closes: Vec<f64> = spy.iter().map(|r| r.close).collect();

    let mut strategies: Vec<StrategyResult> = Vec::new();

    // Buy & Hold
    let bh_equity = buy_and_hold_equity(&closes, args.capital);
    strategies.push(StrategyResult {
        name: "Buy & Hold".to_string(),
        equity: bh_equity,
    });

    // Intraday
    let mask_all = vec![true; n];
    let eq_intra = simulate_strategy(
        &opens,
        &closes,
        &mask_all,
        args.capital,
        &ibkr_roundtrip_cost,
        args.short,
    );
    let label = if args.short {
        "Short Open→Cover Close"
    } else {
        "Intraday (Open→Close)"
    };
    strategies.push(StrategyResult {
        name: label.to_string(),
        equity: eq_intra,
    });

    let years = (dates
        .last()
        .unwrap()
        .signed_duration_since(*dates.first().unwrap()))
    .num_days() as f64
        / 365.25;

    if json_mode {
        let results: Vec<_> = strategies
            .iter()
            .map(|s| compute_strategy_metrics(&s.name, &s.equity, years))
            .collect();

        let output = JsonOutput {
            strategy: "intraday-drift".to_string(),
            ticker: ticker.clone(),
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
        let mode = if args.short {
            "Short Open, Cover Close"
        } else {
            "Intraday Drift (Open→Close)"
        };
        println!(
            "{}",
            crate::strategies::common::format_results_md(
                &format!("{} — {}", mode, ticker),
                &ticker,
                &dates,
                years,
                args.capital,
                &strategies,
                &[],
                args.start_year_table,
            )
        );
    } else {
        println!();
        println!("{}", "=".repeat(80));
        let mode = if args.short {
            "SHORT OPEN, COVER CLOSE"
        } else {
            "BUY THE OPEN, SELL THE CLOSE"
        };
        println!("{} — {} BACKTEST RESULTS", mode, ticker);
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
        }

        println!("\nAnnual Returns (from {}):", args.start_year_table);
        println!(
            "{}",
            format_annual_table(&strategies, &dates, args.start_year_table, 25)
        );

        if !args.no_plots {
            println!("\nPlotting not yet implemented in Rust port (use --no-plots)");
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
    fn test_compute_intraday_returns() {
        let rows = make_rows(&[100.0, 102.0, 104.0], &[101.0, 103.0, 105.0]);
        let ret = compute_intraday_returns(&rows);
        assert!((ret[0] - (101.0_f64 / 100.0).ln()).abs() < 1e-10);
        assert!((ret[1] - (103.0_f64 / 102.0).ln()).abs() < 1e-10);
        assert!((ret[2] - (105.0_f64 / 104.0).ln()).abs() < 1e-10);
    }

    #[test]
    fn test_simulate_basic() {
        let opens = [100.0, 101.0, 102.0];
        let closes = [101.0, 102.0, 103.0];
        let mask = [true, true, true];

        let equity = simulate_strategy(&opens, &closes, &mask, 10_000.0, &|_, _| 0.0, false);
        assert_eq!(equity.len(), 4);
        assert_eq!(equity[0], 10_000.0);
        assert!((equity[1] - 10_100.0).abs() < 1e-6);
    }

    #[test]
    fn test_mask_skips_trades() {
        let opens = [100.0, 100.0];
        let closes = [110.0, 110.0];
        let mask = [false, true];

        let equity = simulate_strategy(&opens, &closes, &mask, 10_000.0, &|_, _| 0.0, false);
        assert_eq!(equity[1], 10_000.0);
        assert!(equity[2] > 10_000.0);
    }

    #[test]
    fn test_loss_day() {
        let opens = [100.0];
        let closes = [95.0];
        let mask = [true];

        let equity = simulate_strategy(&opens, &closes, &mask, 10_000.0, &|_, _| 0.0, false);
        assert!((equity[1] - 9_500.0).abs() < 1e-6);
    }

    #[test]
    fn test_fees_reduce_equity() {
        let opens = [100.0];
        let closes = [100.0];
        let mask = [true];

        let equity = simulate_strategy(
            &opens,
            &closes,
            &mask,
            10_000.0,
            &ibkr_roundtrip_cost,
            false,
        );
        assert!(equity[1] < 10_000.0);
    }

    #[test]
    fn test_short_direction() {
        let opens = [100.0];
        let closes = [95.0];
        let mask = [true];

        let equity = simulate_strategy(&opens, &closes, &mask, 10_000.0, &|_, _| 0.0, true);
        // Short: pnl = -1 * 100 * (95-100) = 500
        assert!((equity[1] - 10_500.0).abs() < 1e-6);
    }

    #[test]
    fn test_negative_return() {
        let rows = make_rows(&[105.0], &[100.0]);
        let ret = compute_intraday_returns(&rows);
        assert!(ret[0] < 0.0);
    }

    #[test]
    fn test_flat_day() {
        let rows = make_rows(&[100.0], &[100.0]);
        let ret = compute_intraday_returns(&rows);
        assert!((ret[0]).abs() < 1e-10);
    }
}
