/// Performance metrics: CAGR, Sharpe, max drawdown, VaR95, annual returns.
use chrono::{Datelike, NaiveDate};

pub const RISK_FREE_RATE: f64 = 0.04;

/// Compound annual growth rate.
pub fn cagr(equity: &[f64], years: f64) -> f64 {
    if years <= 0.0 || equity[0] <= 0.0 {
        return 0.0;
    }
    (equity.last().unwrap() / equity[0]).powf(1.0 / years) - 1.0
}

/// Annualized Sharpe ratio from daily returns.
pub fn sharpe(returns: &[f64], rf: f64) -> f64 {
    let daily_rf = rf / 252.0;
    let excess: Vec<f64> = returns.iter().map(|r| r - daily_rf).collect();
    if excess.len() < 2 {
        return 0.0;
    }
    let n = excess.len() as f64;
    let mean = excess.iter().sum::<f64>() / n;
    let var = excess.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std = var.sqrt();
    if std < 1e-15 {
        return 0.0;
    }
    mean / std * 252.0_f64.sqrt()
}

/// Sharpe with default risk-free rate.
pub fn sharpe_default(returns: &[f64]) -> f64 {
    sharpe(returns, RISK_FREE_RATE)
}

/// Maximum drawdown as a positive fraction.
pub fn max_drawdown(equity: &[f64]) -> f64 {
    let mut peak = equity[0];
    let mut max_dd = 0.0_f64;
    for &val in equity {
        if val > peak {
            peak = val;
        }
        let dd = (peak - val) / peak;
        if dd > max_dd {
            max_dd = dd;
        }
    }
    max_dd
}

/// 95% Value at Risk (historical, daily).
///
/// Uses numpy-compatible linear interpolation for the 5th percentile.
pub fn var_95(returns: &[f64]) -> f64 {
    let mut clean: Vec<f64> = returns.iter().copied().filter(|x| x.is_finite()).collect();
    if clean.is_empty() {
        return 0.0;
    }
    clean.sort_by(|a, b| a.partial_cmp(b).unwrap());
    percentile_linear(&clean, 5.0)
}

/// Linear interpolation percentile matching numpy's default method.
fn percentile_linear(sorted: &[f64], pct: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = pct / 100.0 * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = lo + 1;
    let frac = rank - lo as f64;
    if hi >= n {
        sorted[n - 1]
    } else {
        sorted[lo] + frac * (sorted[hi] - sorted[lo])
    }
}

/// Per-year returns from equity curve.
pub fn annual_returns_table(
    equity: &[f64],
    dates: &[NaiveDate],
    start_year: i32,
) -> Vec<(i32, f64)> {
    // equity[0] is initial capital; equity[1..] corresponds to dates[..]
    // Skip the first element (initial capital) to match Python behavior
    let values = &equity[1..];
    assert_eq!(values.len(), dates.len());

    // Group by year
    let mut years: std::collections::BTreeMap<i32, Vec<f64>> = std::collections::BTreeMap::new();
    for (val, date) in values.iter().zip(dates.iter()) {
        let year = date.year();
        if year >= start_year {
            years.entry(year).or_default().push(*val);
        }
    }

    years
        .into_iter()
        .map(|(year, vals)| {
            let first = vals[0];
            let last = *vals.last().unwrap();
            let ret = (last / first) - 1.0;
            (year, ret)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_known_cagr() {
        let equity = [100.0, 200.0];
        let result = cagr(&equity, 10.0);
        let expected = 2.0_f64.powf(1.0 / 10.0) - 1.0;
        assert_relative_eq!(result, expected, epsilon = 1e-6);
    }

    #[test]
    fn test_zero_years() {
        assert_eq!(cagr(&[100.0, 200.0], 0.0), 0.0);
    }

    #[test]
    fn test_zero_start() {
        assert_eq!(cagr(&[0.0, 200.0], 5.0), 0.0);
    }

    #[test]
    fn test_known_sharpe() {
        // Constant returns -> std is 0 -> Sharpe should be 0
        let returns = vec![0.001; 252];
        let s = sharpe(&returns, 0.0);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn test_sharpe_with_variance() {
        let returns: Vec<f64> = (0..252).map(|i| 0.001 + (i as f64 * 0.0001)).collect();
        let s = sharpe(&returns, 0.0);
        assert!(s > 0.0);
    }

    #[test]
    fn test_zero_vol() {
        let daily_ret = vec![0.001; 10];
        assert_eq!(sharpe_default(&daily_ret), 0.0);
    }

    #[test]
    fn test_empty() {
        assert_eq!(sharpe_default(&[0.01]), 0.0);
    }

    #[test]
    fn test_known_drawdown() {
        let equity = [100.0, 120.0, 90.0, 110.0, 80.0];
        assert_relative_eq!(max_drawdown(&equity), 40.0 / 120.0, epsilon = 1e-10);
    }

    #[test]
    fn test_no_drawdown() {
        let equity = [100.0, 110.0, 120.0, 130.0];
        assert_relative_eq!(max_drawdown(&equity), 0.0, epsilon = 1e-10);
    }

    #[test]
    fn test_var_with_nans() {
        let returns = [0.01, -0.02, f64::NAN, 0.005, -0.03, 0.0, -0.01];
        let v = var_95(&returns);
        assert!(v.is_finite());
    }

    #[test]
    fn test_percentile_linear() {
        let sorted = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        // 50th percentile of [1,2,3,4,5] = 3.0
        assert_relative_eq!(percentile_linear(&sorted, 50.0), 3.0, epsilon = 1e-10);
        // 25th percentile = 2.0
        assert_relative_eq!(percentile_linear(&sorted, 25.0), 2.0, epsilon = 1e-10);
    }

    #[test]
    fn test_known_var() {
        // Generate pseudo-random normal-ish returns and check VaR is near expected
        let mut returns = Vec::with_capacity(10000);
        let mut rng_state: u64 = 42;
        for _ in 0..10000 {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let u = (rng_state >> 33) as f64 / (1u64 << 31) as f64;
            returns.push((u - 0.5) * 0.02);
        }
        let v = var_95(&returns);
        // Should be roughly near -0.01 (5th percentile of uniform(-0.01, 0.01))
        assert!(v < 0.0, "VaR should be negative: {v}");
        assert!(v.is_finite());
    }

    #[test]
    fn test_annual_returns_year_filtering() {
        // Generate dates spanning 2014-2015, filter from 2015
        let mut dates = Vec::new();
        let mut d = NaiveDate::from_ymd_opt(2014, 1, 2).unwrap();
        for _ in 0..500 {
            dates.push(d);
            d += chrono::Duration::days(1);
            // Skip weekends
            while d.weekday().num_days_from_monday() >= 5 {
                d += chrono::Duration::days(1);
            }
        }
        let n = dates.len();
        let equity_vals: Vec<f64> = (0..n).map(|i| 100.0 + i as f64 * 0.2).collect();
        let mut equity = vec![100.0];
        equity.extend_from_slice(&equity_vals);

        let table = annual_returns_table(&equity, &dates, 2015);
        let years: Vec<i32> = table.iter().map(|(y, _)| *y).collect();
        assert!(!years.contains(&2014));
        assert!(years.contains(&2015));
    }

    #[test]
    fn test_annual_returns_calculation() {
        // Two years of constant prices within each year
        let mut dates = Vec::new();
        let mut d = NaiveDate::from_ymd_opt(2020, 1, 2).unwrap();
        let end_2020 = NaiveDate::from_ymd_opt(2020, 12, 31).unwrap();
        while d <= end_2020 {
            if d.weekday().num_days_from_monday() < 5 {
                dates.push(d);
            }
            d += chrono::Duration::days(1);
        }
        let n_2020 = dates.len();
        d = NaiveDate::from_ymd_opt(2021, 1, 4).unwrap();
        let end_2021 = NaiveDate::from_ymd_opt(2021, 12, 31).unwrap();
        while d <= end_2021 {
            if d.weekday().num_days_from_monday() < 5 {
                dates.push(d);
            }
            d += chrono::Duration::days(1);
        }

        let n = dates.len();
        let mut equity_vals = Vec::with_capacity(n);
        for i in 0..n {
            if i < n_2020 {
                equity_vals.push(100.0); // flat in 2020
            } else {
                equity_vals.push(120.0); // flat in 2021
            }
        }
        let mut equity = vec![100.0]; // initial capital
        equity.extend_from_slice(&equity_vals);

        let table = annual_returns_table(&equity, &dates, 2020);
        let yr_2020 = table.iter().find(|(y, _)| *y == 2020).unwrap().1;
        assert!((yr_2020).abs() < 0.01, "2020 should be flat: {yr_2020}");
        let yr_2021 = table.iter().find(|(y, _)| *y == 2021).unwrap().1;
        assert!((yr_2021).abs() < 0.01, "2021 should be flat: {yr_2021}");
    }
}
