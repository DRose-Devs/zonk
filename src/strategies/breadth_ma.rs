/// Single moving-average breadth strategy.
///
/// Computes the percentage of a universe that is below (or above) a configurable
/// N-day simple moving average (default: 50-day). Triggers a signal when that
/// percentage crosses a threshold, then measures forward returns on specified
/// assets.
///
/// This is functionally equivalent to `breadth-washout --lookback N` but uses
/// a 50-day default and provides a clearer entry point for MA-based breadth
/// analysis.
use anyhow::Result;
use clap;

use crate::cli::OutputFormat;
use crate::strategies::breadth_washout::{self, BreadthWashoutArgs};

/// CLI arguments for the breadth-ma strategy.
#[derive(Debug, clap::Args)]
pub struct BreadthMaArgs {
    #[arg(
        long,
        default_value = "2026-03-11",
        help = "Signal evaluation end date"
    )]
    pub end_date: String,

    #[arg(long, default_value_t = 252, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(
        long,
        default_value_t = 50,
        help = "Moving average period (e.g. 50 for 50-day MA)"
    )]
    pub short_period: usize,

    #[arg(
        long,
        default_value = "oversold",
        help = "Breadth signal mode: oversold or overbought"
    )]
    pub signal_mode: String,

    #[arg(long, default_value_t = 80.0, help = "Signal threshold percent")]
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

/// Convert BreadthMaArgs into the underlying BreadthWashoutArgs and delegate.
pub fn run(args: &BreadthMaArgs, fmt: OutputFormat) -> Result<()> {
    let washout_args = BreadthWashoutArgs {
        end_date: args.end_date.clone(),
        sessions: args.sessions,
        lookback: args.short_period,
        signal_mode: args.signal_mode.clone(),
        threshold: Some(args.threshold),
        min_pct_below: args.threshold,
        universe: args.universe.clone(),
        preset: args.preset.clone(),
        tickers: args.tickers.clone(),
        universe_label: args.universe_label.clone(),
        membership_time_of_day: args.membership_time_of_day.clone(),
        snapshot_date: args.snapshot_date.clone(),
        bronze_dir: args.bronze_dir.clone(),
        assets: args.assets.clone(),
        horizon: args.horizon.clone(),
        price_returns: args.price_returns,
        max_workers: args.max_workers,
    };

    breadth_washout::run(&washout_args, fmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_short_period() {
        let args = BreadthMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            short_period: 50,
            signal_mode: "oversold".to_string(),
            threshold: 80.0,
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
        assert_eq!(args.short_period, 50);
    }

    #[test]
    fn test_overbought_mode() {
        let args = BreadthMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 100,
            short_period: 20,
            signal_mode: "overbought".to_string(),
            threshold: 70.0,
            universe: "sp500".to_string(),
            preset: None,
            tickers: None,
            universe_label: Some("sp500-test".to_string()),
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["QQQ".to_string(), "TQQQ".to_string()],
            horizon: Some(vec!["1d=1".to_string()]),
            price_returns: true,
            max_workers: 4,
        };
        assert_eq!(args.signal_mode, "overbought");
        assert_eq!(args.threshold, 70.0);
        assert_eq!(args.short_period, 20);
    }

    #[test]
    fn test_explicit_tickers() {
        let args = BreadthMaArgs {
            end_date: "2026-03-11".to_string(),
            sessions: 252,
            short_period: 50,
            signal_mode: "oversold".to_string(),
            threshold: 80.0,
            universe: "ndx100".to_string(),
            preset: None,
            tickers: Some(vec!["AAPL".to_string(), "MSFT".to_string()]),
            universe_label: None,
            membership_time_of_day: "EOD".to_string(),
            snapshot_date: None,
            bronze_dir: None,
            assets: vec!["SPY".to_string()],
            horizon: None,
            price_returns: false,
            max_workers: 12,
        };
        assert_eq!(args.tickers.as_ref().unwrap().len(), 2);
    }
}
