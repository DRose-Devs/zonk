/// Shared strategy utilities: buy-and-hold, report formatting, daily returns, JSON output.
use chrono::NaiveDate;
use num_format::{Locale, ToFormattedString};
use serde::Serialize;
use serde_json::Value;

use crate::metrics::fees::ibkr_roundtrip_cost;
use crate::metrics::performance::{
    annual_returns_table, cagr, max_drawdown, sharpe_default, var_95,
};

/// Compute daily returns from an equity curve.
pub fn daily_returns(equity: &[f64]) -> Vec<f64> {
    equity.windows(2).map(|w| (w[1] - w[0]) / w[0]).collect()
}

/// Simulate buy-and-hold equity curve.
pub fn buy_and_hold_equity(closes: &[f64], capital: f64) -> Vec<f64> {
    let n = closes.len();
    let shares = (capital / closes[0]) as i64;
    let cost = ibkr_roundtrip_cost(capital, closes[0]);
    let cash_remainder = capital - shares as f64 * closes[0];

    let mut equity = Vec::with_capacity(n + 1);
    equity.push(capital);
    for i in 0..n {
        equity.push(shares as f64 * closes[i] + cash_remainder - cost);
    }
    equity
}

/// Strategy result with equity curve and name.
pub struct StrategyResult {
    pub name: String,
    pub equity: Vec<f64>,
}

/// Format the standard results table header.
pub fn format_results_header() -> (String, String) {
    let header = format!(
        "{:<25} {:>14} {:>8} {:>8} {:>8} {:>8}",
        "Strategy", "Final ($)", "CAGR", "Sharpe", "MaxDD", "VaR95"
    );
    let sep = "-".repeat(header.len());
    (header, sep)
}

/// Format a single strategy row in the results table.
pub fn format_strategy_row(name: &str, equity: &[f64], years: f64) -> String {
    let dr = daily_returns(equity);
    let c = cagr(equity, years);
    let s = sharpe_default(&dr);
    let md = max_drawdown(equity);
    let v = var_95(&dr);

    let final_val = *equity.last().unwrap() as i64;
    let final_str = final_val.to_formatted_string(&Locale::en);

    format!(
        "{:<25} {:>14} {:>7.1}% {:>8.2} {:>7.1}% {:>8.4}",
        name,
        final_str,
        c * 100.0,
        s,
        md * 100.0,
        v
    )
}

/// Format annual returns table for multiple strategies.
pub fn format_annual_table(
    strategies: &[StrategyResult],
    dates: &[NaiveDate],
    start_year: i32,
    col_width: usize,
) -> String {
    let mut lines = Vec::new();

    // Header
    let mut header = format!("{:<6}", "Year");
    for strat in strategies {
        header += &format!(" {:>width$}", strat.name, width = col_width);
    }
    lines.push(header.clone());
    lines.push("-".repeat(header.len()));

    // Compute tables
    let tables: Vec<Vec<(i32, f64)>> = strategies
        .iter()
        .map(|s| annual_returns_table(&s.equity, dates, start_year))
        .collect();

    // Collect all years
    let mut all_years: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for tbl in &tables {
        for (year, _) in tbl {
            all_years.insert(*year);
        }
    }

    for year in all_years {
        let mut row = format!("{:<6}", year);
        for tbl in &tables {
            if let Some((_, ret)) = tbl.iter().find(|(y, _)| *y == year) {
                row += &format!(" {:>width$.1}%", ret * 100.0, width = col_width - 1);
            } else {
                row += &format!(" {:>width$}", "N/A", width = col_width);
            }
        }
        lines.push(row);
    }

    lines.join("\n")
}

/// Metrics for a single strategy in JSON output.
#[derive(Debug, Serialize)]
pub struct JsonStrategyMetrics {
    pub name: String,
    pub beginning_equity: f64,
    pub final_equity: f64,
    pub cagr: f64,
    pub sharpe: f64,
    pub max_drawdown: f64,
    pub var_95: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adf_stat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adf_p_value: Option<f64>,
}

/// Annual return entry.
#[derive(Debug, Serialize)]
pub struct JsonAnnualReturn {
    pub year: i32,
    pub returns: std::collections::BTreeMap<String, f64>,
}

/// Top-level JSON output for backtest strategies.
#[derive(Debug, Serialize)]
pub struct JsonOutput {
    pub strategy: String,
    pub ticker: String,
    pub period_start: String,
    pub period_end: String,
    pub years: f64,
    pub capital: f64,
    pub fee_model: String,
    pub results: Vec<JsonStrategyMetrics>,
    pub annual_returns: Vec<JsonAnnualReturn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<Value>,
}

/// Compute metrics for a strategy and return as JsonStrategyMetrics.
pub fn compute_strategy_metrics(name: &str, equity: &[f64], years: f64) -> JsonStrategyMetrics {
    let dr = daily_returns(equity);
    JsonStrategyMetrics {
        name: name.to_string(),
        beginning_equity: *equity.first().unwrap_or(&0.0),
        final_equity: *equity.last().unwrap(),
        cagr: cagr(equity, years),
        sharpe: sharpe_default(&dr),
        max_drawdown: max_drawdown(equity),
        var_95: var_95(&dr),
        adf_stat: None,
        adf_p_value: None,
    }
}

/// Build annual returns for JSON output.
pub fn build_json_annual_returns(
    strategies: &[StrategyResult],
    dates: &[NaiveDate],
    start_year: i32,
) -> Vec<JsonAnnualReturn> {
    let tables: Vec<Vec<(i32, f64)>> = strategies
        .iter()
        .map(|s| annual_returns_table(&s.equity, dates, start_year))
        .collect();

    let mut all_years: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for tbl in &tables {
        for (year, _) in tbl {
            all_years.insert(*year);
        }
    }

    all_years
        .into_iter()
        .map(|year| {
            let mut returns = std::collections::BTreeMap::new();
            for (i, tbl) in tables.iter().enumerate() {
                if let Some((_, ret)) = tbl.iter().find(|(y, _)| *y == year) {
                    returns.insert(strategies[i].name.clone(), *ret);
                }
            }
            JsonAnnualReturn { year, returns }
        })
        .collect()
}

/// Format backtest results as a markdown table.
pub fn format_results_md(
    title: &str,
    ticker: &str,
    dates: &[NaiveDate],
    years: f64,
    capital: f64,
    strategies: &[StrategyResult],
    adf_results: &[(String, f64, f64)],
    start_year: i32,
) -> String {
    let mut lines = Vec::new();

    lines.push(format!("# {title}"));
    lines.push(String::new());
    lines.push(format!(
        "**Ticker:** {} | **Period:** {} to {} ({:.1} years) | **Capital:** ${:.0}  ",
        ticker,
        dates.first().unwrap(),
        dates.last().unwrap(),
        years,
        capital,
    ));
    lines.push(String::new());
    lines.push("## Results".to_string());
    lines.push(String::new());
    lines.push("| Strategy | Final ($) | CAGR | Sharpe | Max DD | VaR 95 |".to_string());
    lines.push("|----------|-----------|------|--------|--------|--------|".to_string());

    for strat in strategies {
        let dr = daily_returns(&strat.equity);
        let c = cagr(&strat.equity, years);
        let s = sharpe_default(&dr);
        let md = max_drawdown(&strat.equity);
        let v = var_95(&dr);
        let final_val = *strat.equity.last().unwrap() as i64;
        let final_str = final_val.to_formatted_string(&Locale::en);

        let mut row = format!(
            "| {} | {} | {:.1}% | {:.2} | {:.1}% | {:.4} |",
            strat.name,
            final_str,
            c * 100.0,
            s,
            md * 100.0,
            v,
        );

        if let Some((_, stat, pval)) = adf_results.iter().find(|(n, _, _)| *n == strat.name) {
            row = format!(
                "| {} | {} | {:.1}% | {:.2} | {:.1}% | {:.4} | ADF: {:.4} (p={:.6})",
                strat.name,
                final_str,
                c * 100.0,
                s,
                md * 100.0,
                v,
                stat,
                pval,
            );
        }

        lines.push(row);
    }

    // Annual returns
    let tables: Vec<Vec<(i32, f64)>> = strategies
        .iter()
        .map(|s| annual_returns_table(&s.equity, dates, start_year))
        .collect();

    let mut all_years: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for tbl in &tables {
        for (year, _) in tbl {
            all_years.insert(*year);
        }
    }

    if !all_years.is_empty() {
        lines.push(String::new());
        lines.push(format!("## Annual Returns (from {})", start_year));
        lines.push(String::new());
        let mut header = "| Year |".to_string();
        let mut sep = "|------|".to_string();
        for strat in strategies {
            header += &format!(" {} |", strat.name);
            sep += &format!("{}|", "-".repeat(strat.name.len() + 2));
        }
        lines.push(header);
        lines.push(sep);

        for year in &all_years {
            let mut row = format!("| {} |", year);
            for tbl in &tables {
                if let Some((_, ret)) = tbl.iter().find(|(y, _)| y == year) {
                    row += &format!(" {:.1}% |", ret * 100.0);
                } else {
                    row += " N/A |";
                }
            }
            lines.push(row);
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daily_returns() {
        let equity = [100.0, 110.0, 105.0];
        let dr = daily_returns(&equity);
        assert_eq!(dr.len(), 2);
        assert!((dr[0] - 0.1).abs() < 1e-10);
        assert!((dr[1] - (-5.0 / 110.0)).abs() < 1e-10);
    }

    #[test]
    fn test_buy_and_hold() {
        let closes = [100.0, 110.0, 105.0];
        let equity = buy_and_hold_equity(&closes, 10_000.0);
        assert_eq!(equity.len(), 4);
        assert_eq!(equity[0], 10_000.0);
        // equity[1] = shares * closes[0] + cash_remainder - cost (purchase day)
        // equity[2] = shares * closes[1] + cash_remainder - cost
        let shares = 100i64;
        let cost = ibkr_roundtrip_cost(10_000.0, 100.0);
        let expected_day2 = shares as f64 * 110.0 + (10_000.0 - shares as f64 * 100.0) - cost;
        assert!(
            (equity[2] - expected_day2).abs() < 1e-6,
            "equity[2]={}, expected={}, cost={}",
            equity[2],
            expected_day2,
            cost
        );
    }

    #[test]
    fn test_format_strategy_row_produces_output() {
        let equity = [1_000_000.0, 1_100_000.0, 1_050_000.0, 1_200_000.0];
        let row = format_strategy_row("Test Strategy", &equity, 1.0);
        assert!(row.contains("Test Strategy"));
        assert!(row.contains("1,200,000"));
    }

    #[test]
    fn test_compute_strategy_metrics() {
        let equity = [1_000_000.0, 1_100_000.0, 1_050_000.0, 1_200_000.0];
        let m = compute_strategy_metrics("Test", &equity, 1.0);
        assert_eq!(m.name, "Test");
        assert_eq!(m.beginning_equity, 1_000_000.0);
        assert_eq!(m.final_equity, 1_200_000.0);
        assert!(m.cagr > 0.0);
        assert!(m.max_drawdown > 0.0);
        assert!(m.adf_stat.is_none());
        assert!(m.adf_p_value.is_none());
    }

    #[test]
    fn test_compute_strategy_metrics_serializes_to_json() {
        let equity = [1_000_000.0, 1_100_000.0, 1_200_000.0];
        let m = compute_strategy_metrics("Buy & Hold", &equity, 1.0);
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"name\":\"Buy & Hold\""));
        assert!(json.contains("\"beginning_equity\":1000000.0"));
        assert!(json.contains("\"final_equity\":1200000.0"));
        // adf fields should be omitted (skip_serializing_if = None)
        assert!(!json.contains("adf_stat"));
    }

    #[test]
    fn test_build_json_annual_returns() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2020, 6, 1).unwrap(),
            NaiveDate::from_ymd_opt(2020, 12, 31).unwrap(),
            NaiveDate::from_ymd_opt(2021, 6, 1).unwrap(),
        ];
        let strategies = vec![StrategyResult {
            name: "Test".to_string(),
            equity: vec![100.0, 100.0, 110.0, 120.0],
        }];
        let annual = build_json_annual_returns(&strategies, &dates, 2020);
        assert_eq!(annual.len(), 2);
        assert_eq!(annual[0].year, 2020);
        assert!(annual[0].returns.contains_key("Test"));
    }
}
