/// NASDAQ-100 5-day SMA breadth analysis.
///
/// Computes for each trading session how many NDX-100 members closed above
/// their 5-day SMA and summarizes the distribution.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::cli::OutputFormat;
use crate::config::presets_dir;
use crate::data::readers::load_close_frame;

pub const DEFAULT_FORWARD_HORIZONS: &[(&str, usize)] =
    &[("1d", 1), ("1w", 5), ("1m", 21), ("3m", 63)];

fn default_preset() -> PathBuf {
    presets_dir().join("ndx100.json")
}

fn default_warehouse() -> PathBuf {
    dirs::home_dir().unwrap().join("market-warehouse")
}

/// Load the ticker universe from a preset JSON file.
pub fn load_universe(preset_path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(preset_path)
        .with_context(|| format!("reading preset {}", preset_path.display()))?;

    #[derive(Deserialize)]
    struct Payload {
        tickers: Option<Vec<String>>,
    }
    let payload: Payload = serde_json::from_str(&content)?;
    match payload.tickers {
        Some(ref t) if !t.is_empty() => Ok(t.iter().map(|s| s.to_uppercase()).collect()),
        _ => bail!(
            "Preset {} does not contain a non-empty ticker list",
            preset_path.display()
        ),
    }
}

/// Compute daily breadth counts and percentages from a close-price matrix.
pub fn compute_breadth(
    dates: &[NaiveDate],
    symbols: &[String],
    prices: &HashMap<String, Vec<(NaiveDate, f64)>>,
    lookback: usize,
    universe_size: usize,
) -> Result<Vec<BreadthRow>> {
    if lookback == 0 {
        bail!("lookback must be positive");
    }

    // Build price matrix indexed by (date, symbol)
    let mut price_map: HashMap<(NaiveDate, &str), f64> = HashMap::new();
    for (sym, data) in prices {
        for (d, p) in data {
            price_map.insert((*d, sym.as_str()), *p);
        }
    }

    // Compute rolling SMA for each symbol
    let mut sma_map: HashMap<(NaiveDate, &str), f64> = HashMap::new();
    for sym in symbols {
        let mut sym_prices: Vec<(NaiveDate, f64)> = prices.get(sym).cloned().unwrap_or_default();
        sym_prices.sort_by_key(|(d, _)| *d);

        for i in 0..sym_prices.len() {
            if i + 1 >= lookback {
                let sum: f64 = sym_prices[i + 1 - lookback..=i]
                    .iter()
                    .map(|(_, p)| p)
                    .sum();
                sma_map.insert((sym_prices[i].0, sym.as_str()), sum / lookback as f64);
            }
        }
    }

    let mut result = Vec::new();
    for &date in dates {
        let mut eligible_count = 0;
        let mut above_count = 0;

        for sym in symbols {
            if let (Some(&price), Some(&sma)) = (
                price_map.get(&(date, sym.as_str())),
                sma_map.get(&(date, sym.as_str())),
            ) {
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

    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
pub struct BreadthRow {
    pub trade_date: NaiveDate,
    pub eligible_count: usize,
    pub above_count: usize,
    pub below_or_equal_count: usize,
    pub unavailable_count: usize,
    pub pct_above: f64,
    pub pct_below_or_equal: f64,
}

/// Select trailing N sessions through the requested end date.
pub fn select_trailing_sessions(
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

/// Summarize a breadth-percentage series with common distribution stats.
pub fn summarize_distribution(values: &[f64]) -> Result<DistributionSummary> {
    let clean: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if clean.is_empty() {
        bail!("Cannot summarize an empty series");
    }

    let mut sorted = clean.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = sorted.len();
    let mean = sorted.iter().sum::<f64>() / n as f64;
    let std = if n > 1 {
        let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        variance.sqrt()
    } else {
        0.0
    };

    Ok(DistributionSummary {
        observations: n,
        mean,
        std,
        min: sorted[0],
        p05: percentile(&sorted, 5.0),
        p10: percentile(&sorted, 10.0),
        p25: percentile(&sorted, 25.0),
        median: percentile(&sorted, 50.0),
        p75: percentile(&sorted, 75.0),
        p90: percentile(&sorted, 90.0),
        p95: percentile(&sorted, 95.0),
        max: sorted[n - 1],
    })
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = pct / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = rank - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

#[derive(Debug, Clone, Serialize)]
pub struct DistributionSummary {
    pub observations: usize,
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub p05: f64,
    pub p10: f64,
    pub p25: f64,
    pub median: f64,
    pub p75: f64,
    pub p90: f64,
    pub p95: f64,
    pub max: f64,
}

/// Build histogram table with equal-width bands.
pub fn build_histogram_table(values: &[f64], bin_size: usize) -> Result<Vec<HistogramBin>> {
    if 100 % bin_size != 0 {
        bail!("bin_size must divide 100 evenly");
    }

    let clean: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if clean.is_empty() {
        bail!("Cannot build a histogram from an empty series");
    }

    let n_bins = 100 / bin_size;
    let mut bins = Vec::with_capacity(n_bins);

    for i in 0..n_bins {
        let left = i * bin_size;
        let right = (i + 1) * bin_size;
        let count = clean
            .iter()
            .filter(|&&v| {
                if i == 0 {
                    v >= left as f64 && v <= right as f64
                } else {
                    v > left as f64 && v <= right as f64
                }
            })
            .count();
        bins.push(HistogramBin {
            label: format!("{left}-{right}%"),
            days: count,
            share_of_days_pct: count as f64 / clean.len() as f64 * 100.0,
        });
    }

    Ok(bins)
}

#[derive(Debug, Clone, Serialize)]
pub struct HistogramBin {
    pub label: String,
    pub days: usize,
    pub share_of_days_pct: f64,
}

/// Compute simple close-to-close forward returns for each named horizon.
pub fn compute_forward_returns(
    close_series: &[(NaiveDate, f64)],
    horizons: &[(&str, usize)],
) -> Vec<(NaiveDate, Vec<(String, f64)>)> {
    let n = close_series.len();
    let mut result = Vec::with_capacity(n);

    for i in 0..n {
        let mut horizon_returns = Vec::new();
        for (label, steps) in horizons {
            if i + steps < n {
                let ret = close_series[i + steps].1 / close_series[i].1 - 1.0;
                horizon_returns.push((label.to_string(), ret));
            }
        }
        result.push((close_series[i].0, horizon_returns));
    }

    result
}

/// CLI arguments for the ndx100-sma-breadth strategy.
#[derive(Debug, clap::Args)]
pub struct Ndx100SmaBreadthArgs {
    #[arg(long, help = "Universe preset JSON")]
    pub preset: Option<PathBuf>,

    #[arg(long, help = "Warehouse root path")]
    pub warehouse: Option<PathBuf>,

    #[arg(
        long,
        default_value = "2026-03-11",
        help = "Inclusive analysis end date (YYYY-MM-DD)"
    )]
    pub end_date: String,

    #[arg(long, default_value_t = 252, help = "Trailing trading sessions")]
    pub sessions: usize,

    #[arg(long, default_value_t = 5, help = "Simple moving average lookback")]
    pub lookback: usize,

    #[arg(long, help = "Optional CSV path for trailing daily breadth")]
    pub csv_out: Option<PathBuf>,

    #[arg(long, help = "Optional JSON path for summary metrics")]
    pub json_out: Option<PathBuf>,
}

/// Run the NDX-100 SMA breadth analysis.
pub fn run(args: &Ndx100SmaBreadthArgs, fmt: OutputFormat) -> Result<()> {
    let json_mode = fmt == OutputFormat::Json;
    let quiet = json_mode || fmt == OutputFormat::Md;
    let preset_path = args.preset.clone().unwrap_or_else(default_preset);
    let warehouse = args.warehouse.clone().unwrap_or_else(default_warehouse);
    let end_date = args.end_date.parse::<NaiveDate>()?;

    let tickers = load_universe(&preset_path)?;
    let universe_size = tickers.len();

    // Compute start date
    let buffer_days = (args.sessions * 2).max(args.lookback * 10) as i64;
    let start_date = end_date - chrono::Duration::days(buffer_days);

    if !quiet {
        println!("Loading close prices for {} symbols...", tickers.len());
    }
    let (price_data, missing) =
        load_close_frame(&tickers, Some(&warehouse), Some(start_date), Some(end_date))?;

    if price_data.is_empty() {
        bail!("No price data loaded for the requested universe");
    }

    // Collect all unique dates
    let mut all_dates: BTreeMap<NaiveDate, ()> = BTreeMap::new();
    let mut prices_map: HashMap<String, Vec<(NaiveDate, f64)>> = HashMap::new();
    for (sym, data) in &price_data {
        prices_map.insert(sym.clone(), data.clone());
        for (d, _) in data {
            all_dates.insert(*d, ());
        }
    }
    let dates: Vec<NaiveDate> = all_dates.keys().copied().collect();
    let symbols: Vec<String> = price_data.iter().map(|(s, _)| s.clone()).collect();

    if !quiet {
        println!("Computing breadth...");
    }
    let breadth = compute_breadth(&dates, &symbols, &prices_map, args.lookback, universe_size)?;
    let trailing = select_trailing_sessions(&breadth, end_date, args.sessions)?;

    let target_row = trailing
        .iter()
        .find(|r| r.trade_date == end_date)
        .context("end date not found in trailing data")?;

    let pct_above_values: Vec<f64> = trailing.iter().map(|r| r.pct_above).collect();
    let summary = summarize_distribution(&pct_above_values)?;
    let histogram = build_histogram_table(&pct_above_values, 10)?;

    if json_mode {
        let output = serde_json::json!({
            "strategy": "ndx100-sma-breadth",
            "universe_size": universe_size,
            "lookback": args.lookback,
            "sessions": trailing.len(),
            "period_start": trailing.first().unwrap().trade_date.to_string(),
            "period_end": trailing.last().unwrap().trade_date.to_string(),
            "missing_symbols": missing,
            "current": {
                "trade_date": target_row.trade_date.to_string(),
                "above_count": target_row.above_count,
                "below_or_equal_count": target_row.below_or_equal_count,
                "unavailable_count": target_row.unavailable_count,
                "pct_above": target_row.pct_above,
                "pct_below_or_equal": target_row.pct_below_or_equal,
            },
            "distribution": summary,
            "histogram": histogram,
            "trailing": trailing,
        });
        println!("{}", serde_json::to_string(&output)?);
    } else if fmt == OutputFormat::Md {
        let mut lines = vec![
            "# NASDAQ-100 Breadth Report".to_string(),
            String::new(),
            format!("**Universe size:** {}  ", universe_size),
            format!(
                "**Window:** {} to {} ({} sessions)  ",
                trailing.first().unwrap().trade_date,
                trailing.last().unwrap().trade_date,
                trailing.len()
            ),
            format!("**Signal:** close > {}-day SMA  ", args.lookback),
        ];

        if !missing.is_empty() {
            lines.push(format!(
                "**Missing:** {} ({})",
                missing.len(),
                missing.join(", ")
            ));
        }

        lines.push(String::new());
        lines.push(format!("## As of {}", target_row.trade_date));
        lines.push(String::new());
        lines.push(format!(
            "- **Above {}-day SMA:** {} ({:.2}%)",
            args.lookback, target_row.above_count, target_row.pct_above
        ));
        lines.push(format!(
            "- **At or below {}-day SMA:** {} ({:.2}%)",
            args.lookback, target_row.below_or_equal_count, target_row.pct_below_or_equal
        ));
        lines.push(format!(
            "- **Unavailable:** {}",
            target_row.unavailable_count
        ));

        lines.push(String::new());
        lines.push("## Distribution".to_string());
        lines.push(String::new());
        lines.push(format!("| Stat | Value |"));
        lines.push(format!("|------|-------|"));
        lines.push(format!("| Mean | {:.2}% |", summary.mean));
        lines.push(format!("| Median | {:.2}% |", summary.median));
        lines.push(format!("| Std Dev | {:.2} pts |", summary.std));
        lines.push(format!(
            "| Min / Max | {:.2}% / {:.2}% |",
            summary.min, summary.max
        ));
        lines.push(format!(
            "| P05 / P10 / P25 | {:.2}% / {:.2}% / {:.2}% |",
            summary.p05, summary.p10, summary.p25
        ));
        lines.push(format!(
            "| P75 / P90 / P95 | {:.2}% / {:.2}% / {:.2}% |",
            summary.p75, summary.p90, summary.p95
        ));

        lines.push(String::new());
        lines.push("## Breadth Histogram".to_string());
        lines.push(String::new());
        lines.push("| Breadth Band | Days | Share % |".to_string());
        lines.push("|--------------|------|---------|".to_string());
        for bin in &histogram {
            lines.push(format!(
                "| {} | {} | {:.1} |",
                bin.label, bin.days, bin.share_of_days_pct
            ));
        }

        println!("{}", lines.join("\n"));
    } else {
        // Format report
        println!("NASDAQ-100 Breadth Report");
        println!("Universe size: {}", universe_size);
        println!(
            "Window: {} to {} ({} sessions)",
            trailing.first().unwrap().trade_date,
            trailing.last().unwrap().trade_date,
            trailing.len()
        );
        println!("Signal: close > {}-day SMA", args.lookback);

        if missing.is_empty() {
            println!("Missing parquet symbols: 0");
        } else {
            println!(
                "Missing parquet symbols: {} ({})",
                missing.len(),
                missing.join(", ")
            );
        }

        println!();
        println!("As of {}", target_row.trade_date);
        println!(
            "Above {}-day SMA: {} ({:.2}%)",
            args.lookback, target_row.above_count, target_row.pct_above
        );
        println!(
            "At or below {}-day SMA: {} ({:.2}%)",
            args.lookback, target_row.below_or_equal_count, target_row.pct_below_or_equal
        );
        println!("Unavailable: {}", target_row.unavailable_count);

        println!();
        println!("Trailing distribution for daily % above 5-day SMA");
        println!("Mean: {:.2}%", summary.mean);
        println!("Median: {:.2}%", summary.median);
        println!("Std dev: {:.2} pts", summary.std);
        println!("Min / Max: {:.2}% / {:.2}%", summary.min, summary.max);
        println!(
            "P05 / P10 / P25: {:.2}% / {:.2}% / {:.2}%",
            summary.p05, summary.p10, summary.p25
        );
        println!(
            "P75 / P90 / P95: {:.2}% / {:.2}% / {:.2}%",
            summary.p75, summary.p90, summary.p95
        );

        println!();
        println!("Breadth histogram");
        println!(
            "{:>15} {:>6} {:>18}",
            "breadth_band", "days", "share_of_days_pct"
        );
        for bin in &histogram {
            println!(
                "{:>15} {:>6} {:>17.1}",
                bin.label, bin.days, bin.share_of_days_pct
            );
        }
    }

    // CSV file output (independent of --output flag)
    if let Some(ref csv_path) = args.csv_out {
        if let Some(parent) = csv_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut wtr = std::fs::File::create(csv_path)?;
        use std::io::Write;
        writeln!(
            wtr,
            "trade_date,eligible_count,above_count,below_or_equal_count,unavailable_count,pct_above,pct_below_or_equal"
        )?;
        for row in &trailing {
            writeln!(
                wtr,
                "{},{},{},{},{},{:.6},{:.6}",
                row.trade_date,
                row.eligible_count,
                row.above_count,
                row.below_or_equal_count,
                row.unavailable_count,
                row.pct_above,
                row.pct_below_or_equal
            )?;
        }
    }

    // JSON file output (independent of --output flag)
    if let Some(ref json_path) = args.json_out {
        if let Some(parent) = json_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::json!({
            "max": summary.max,
            "mean": summary.mean,
            "median": summary.median,
            "min": summary.min,
            "observations": summary.observations,
            "p05": summary.p05,
            "p10": summary.p10,
            "p25": summary.p25,
            "p75": summary.p75,
            "p90": summary.p90,
            "p95": summary.p95,
            "std": summary.std,
        });
        std::fs::write(json_path, serde_json::to_string_pretty(&json)?)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_trailing_sessions() {
        let rows: Vec<BreadthRow> = (0..10)
            .map(|i| BreadthRow {
                trade_date: NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                eligible_count: 100,
                above_count: 50 + i as usize,
                below_or_equal_count: 50 - i as usize,
                unavailable_count: 0,
                pct_above: 50.0 + i as f64,
                pct_below_or_equal: 50.0 - i as f64,
            })
            .collect();

        let end = NaiveDate::from_ymd_opt(2024, 1, 11).unwrap();
        let trailing = select_trailing_sessions(&rows, end, 5).unwrap();
        assert_eq!(trailing.len(), 5);
        assert_eq!(trailing.last().unwrap().trade_date, end);
    }

    #[test]
    fn test_select_trailing_missing_end_date() {
        let rows = vec![BreadthRow {
            trade_date: NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            eligible_count: 100,
            above_count: 50,
            below_or_equal_count: 50,
            unavailable_count: 0,
            pct_above: 50.0,
            pct_below_or_equal: 50.0,
        }];

        let end = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        let result = select_trailing_sessions(&rows, end, 5);
        assert!(result.is_err());
    }

    #[test]
    fn test_summarize_distribution() {
        let values: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        let summary = summarize_distribution(&values).unwrap();
        assert_eq!(summary.observations, 100);
        assert!((summary.mean - 50.5).abs() < 0.01);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.max, 100.0);
    }

    #[test]
    fn test_build_histogram() {
        let values: Vec<f64> = (0..100).map(|i| i as f64 + 0.5).collect();
        let bins = build_histogram_table(&values, 10).unwrap();
        assert_eq!(bins.len(), 10);
        let total_days: usize = bins.iter().map(|b| b.days).sum();
        assert_eq!(total_days, 100);
    }

    #[test]
    fn test_compute_forward_returns() {
        let series: Vec<(NaiveDate, f64)> = (0..10)
            .map(|i| {
                (
                    NaiveDate::from_ymd_opt(2024, 1, 2 + i).unwrap(),
                    100.0 + i as f64,
                )
            })
            .collect();

        let horizons = &[("1d", 1), ("5d", 5)];
        let result = compute_forward_returns(&series, horizons);
        assert_eq!(result.len(), 10);

        // First row: 1d return = 101/100 - 1 = 0.01
        let first = &result[0].1;
        assert_eq!(first.len(), 2);
        assert!((first[0].1 - 0.01).abs() < 1e-10);
    }

    // Helper to create test price data
    fn make_test_prices() -> (
        Vec<NaiveDate>,
        Vec<String>,
        HashMap<String, Vec<(NaiveDate, f64)>>,
    ) {
        let dates = vec![
            NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 3).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 5).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 6).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 9).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 11).unwrap(),
        ];

        let symbols = vec!["AAA".to_string(), "BBB".to_string(), "CCC".to_string()];
        let mut prices = HashMap::new();

        prices.insert(
            "AAA".to_string(),
            dates
                .iter()
                .zip([10.0, 10.0, 10.0, 10.0, 11.0, 12.0, 13.0, 14.0].iter())
                .map(|(d, p)| (*d, *p))
                .collect(),
        );

        prices.insert(
            "BBB".to_string(),
            dates
                .iter()
                .zip([10.0, 10.0, 10.0, 10.0, 9.0, 8.0, 7.0, 6.0].iter())
                .map(|(d, p)| (*d, *p))
                .collect(),
        );

        prices.insert(
            "CCC".to_string(),
            dates
                .iter()
                .zip([10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0].iter())
                .map(|(d, p)| (*d, *p))
                .collect(),
        );

        (dates, symbols, prices)
    }

    #[test]
    fn test_compute_breadth_counts_and_percentages() {
        let (dates, symbols, prices) = make_test_prices();
        let breadth = compute_breadth(&dates, &symbols, &prices, 5, 3).unwrap();

        // Find row for 2026-03-06
        let target = NaiveDate::from_ymd_opt(2026, 3, 6).unwrap();
        let row = breadth.iter().find(|r| r.trade_date == target).unwrap();

        assert_eq!(row.eligible_count, 3);
        assert_eq!(row.above_count, 1); // AAA is above 5-day SMA
        assert_eq!(row.below_or_equal_count, 2); // BBB and CCC at or below
        assert_eq!(row.unavailable_count, 0);
        assert!((row.pct_above - 100.0 / 3.0).abs() < 0.01);
        assert!((row.pct_below_or_equal - 200.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_breadth_with_missing_symbol() {
        let (dates, _symbols, mut prices) = make_test_prices();
        // Only include AAA and BBB in prices but universe_size=3
        prices.remove("CCC");
        let symbols = vec!["AAA".to_string(), "BBB".to_string()];

        let breadth = compute_breadth(&dates, &symbols, &prices, 5, 3).unwrap();

        let target = NaiveDate::from_ymd_opt(2026, 3, 6).unwrap();
        let row = breadth.iter().find(|r| r.trade_date == target).unwrap();

        assert_eq!(row.eligible_count, 2);
        assert_eq!(row.above_count, 1);
        assert_eq!(row.below_or_equal_count, 1);
        assert_eq!(row.unavailable_count, 1);
        assert!((row.pct_above - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_load_universe_from_preset() {
        let tmp = tempfile::tempdir().unwrap();
        let preset = tmp.path().join("test.json");
        std::fs::write(&preset, r#"{"tickers": ["aapl", "msft"]}"#).unwrap();
        let result = load_universe(&preset).unwrap();
        assert_eq!(result, vec!["AAPL", "MSFT"]);
    }

    #[test]
    fn test_load_universe_empty_raises() {
        let tmp = tempfile::tempdir().unwrap();
        let preset = tmp.path().join("empty.json");
        std::fs::write(&preset, r#"{"tickers": []}"#).unwrap();
        let result = load_universe(&preset);
        assert!(result.is_err());
    }

    #[test]
    fn test_summarize_distribution_basic() {
        let values = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let result = summarize_distribution(&values).unwrap();
        assert_eq!(result.observations, 5);
        assert!((result.mean - 30.0).abs() < 0.01);
        assert!((result.min - 10.0).abs() < 0.01);
        assert!((result.max - 50.0).abs() < 0.01);
        assert!((result.median - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_summarize_distribution_empty_raises() {
        let result = summarize_distribution(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_summarize_distribution_single_value() {
        let result = summarize_distribution(&[42.0]).unwrap();
        assert_eq!(result.observations, 1);
        assert!((result.std - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_build_histogram_bad_bin_size() {
        let result = build_histogram_table(&[50.0], 7);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("divide 100 evenly"));
    }

    #[test]
    fn test_build_histogram_empty_raises() {
        let result = build_histogram_table(&[], 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_select_trailing_negative_sessions() {
        let rows = vec![BreadthRow {
            trade_date: NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            eligible_count: 100,
            above_count: 50,
            below_or_equal_count: 50,
            unavailable_count: 0,
            pct_above: 50.0,
            pct_below_or_equal: 50.0,
        }];
        // sessions=0 should fail
        let result =
            select_trailing_sessions(&rows, NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_select_trailing_empty_breadth() {
        let result = select_trailing_sessions(&[], NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), 5);
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_forward_returns_default_horizons() {
        let series: Vec<(NaiveDate, f64)> = (0..100)
            .map(|i| {
                (
                    NaiveDate::from_ymd_opt(2024, 1, 2).unwrap() + chrono::Duration::days(i),
                    100.0 + i as f64,
                )
            })
            .collect();
        let result = compute_forward_returns(&series, DEFAULT_FORWARD_HORIZONS);
        assert_eq!(result.len(), 100);
        // First row should have both 1d and 1w horizons
        let first_horizons: Vec<&str> = result[0].1.iter().map(|(l, _)| l.as_str()).collect();
        assert!(first_horizons.contains(&"1d"));
        assert!(first_horizons.contains(&"1w"));
    }
}
