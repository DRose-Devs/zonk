/// Data loaders for parquet and external sources (VIX from CBOE).
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use polars::prelude::*;

use crate::config::warehouse_root;
use crate::data::paths::parquet_path_for_symbol;

/// OHLCV row extracted from parquet.
#[derive(Debug, Clone)]
pub struct OhlcvRow {
    pub trade_date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Load daily OHLCV bars from bronze parquet.
pub fn load_ticker_ohlcv(
    ticker: &str,
    parquet_path: Option<&Path>,
    warehouse: Option<&Path>,
) -> Result<Vec<OhlcvRow>> {
    let data_file = match parquet_path {
        Some(p) => p.join("data.parquet"),
        None => {
            let root = match warehouse {
                Some(w) => w.to_path_buf(),
                None => warehouse_root()?,
            };
            parquet_path_for_symbol(ticker, Some(&root))?
        }
    };

    if !data_file.exists() {
        bail!("{} parquet not found: {}", ticker, data_file.display());
    }

    let df = LazyFrame::scan_parquet(&data_file, Default::default())?
        .select([
            col("trade_date"),
            col("open"),
            col("high"),
            col("low"),
            col("close"),
            col("volume"),
        ])
        .sort(["trade_date"], Default::default())
        .collect()
        .with_context(|| format!("reading parquet for {ticker}"))?;

    dataframe_to_ohlcv(&df)
}

/// Convert a polars DataFrame with OHLCV columns to Vec<OhlcvRow>.
fn dataframe_to_ohlcv(df: &DataFrame) -> Result<Vec<OhlcvRow>> {
    let dates = df.column("trade_date")?;
    let opens = df.column("open")?.cast(&DataType::Float64)?;
    let highs = df.column("high")?.cast(&DataType::Float64)?;
    let lows = df.column("low")?.cast(&DataType::Float64)?;
    let closes = df.column("close")?.cast(&DataType::Float64)?;
    let volumes = df.column("volume")?.cast(&DataType::Float64)?;

    let opens = opens.f64()?;
    let highs = highs.f64()?;
    let lows = lows.f64()?;
    let closes = closes.f64()?;
    let volumes = volumes.f64()?;

    let n = df.height();
    let mut rows = Vec::with_capacity(n);

    for i in 0..n {
        let trade_date = extract_date(dates, i)?;
        rows.push(OhlcvRow {
            trade_date,
            open: opens.get(i).unwrap_or(f64::NAN),
            high: highs.get(i).unwrap_or(f64::NAN),
            low: lows.get(i).unwrap_or(f64::NAN),
            close: closes.get(i).unwrap_or(f64::NAN),
            volume: volumes.get(i).unwrap_or(0.0),
        });
    }

    Ok(rows)
}

/// Extract a NaiveDate from a polars column at the given index.
fn extract_date(col: &Column, idx: usize) -> Result<NaiveDate> {
    let series = col.as_materialized_series();
    // Try Date type first
    if let Ok(date_chunked) = series.date() {
        if let Some(days) = date_chunked.get(idx) {
            return Ok(chrono::DateTime::from_timestamp(days as i64 * 86400, 0)
                .unwrap()
                .date_naive());
        }
    }
    // Try Datetime
    if let Ok(dt_chunked) = series.datetime() {
        if let Some(ts) = dt_chunked.get(idx) {
            let tu = dt_chunked.time_unit();
            let secs = match tu {
                TimeUnit::Milliseconds => ts / 1000,
                TimeUnit::Microseconds => ts / 1_000_000,
                TimeUnit::Nanoseconds => ts / 1_000_000_000,
            };
            return Ok(chrono::DateTime::from_timestamp(secs, 0)
                .unwrap()
                .date_naive());
        }
    }
    // Try string
    if let Ok(str_chunked) = series.str() {
        if let Some(s) = str_chunked.get(idx) {
            let s = &s[..10]; // YYYY-MM-DD
            return NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .with_context(|| format!("parsing date '{s}'"));
        }
    }
    bail!("Cannot extract date from column at index {idx}")
}

/// Load a trade_date x symbol close-price matrix from bronze parquet.
///
/// Returns (map of symbol -> Vec<(date, close)>, missing symbols).
pub fn load_close_frame(
    symbols: &[String],
    warehouse: Option<&Path>,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<(Vec<(String, Vec<(NaiveDate, f64)>)>, Vec<String>)> {
    let root = match warehouse {
        Some(w) => w.to_path_buf(),
        None => warehouse_root()?,
    };

    let mut series_by_symbol: Vec<(String, Vec<(NaiveDate, f64)>)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for symbol in symbols {
        let data_file = parquet_path_for_symbol(symbol, Some(&root))?;
        if !data_file.exists() {
            missing.push(symbol.clone());
            continue;
        }

        let df = LazyFrame::scan_parquet(&data_file, Default::default())?
            .select([col("trade_date"), col("close")])
            .sort(["trade_date"], Default::default())
            .collect()?;

        let dates_col = df.column("trade_date")?;
        let closes = df.column("close")?.cast(&DataType::Float64)?;
        let closes = closes.f64()?;

        let mut data: Vec<(NaiveDate, f64)> = Vec::new();
        for i in 0..df.height() {
            let date = extract_date(dates_col, i)?;
            if let Some(sd) = start_date {
                if date < sd {
                    continue;
                }
            }
            if let Some(ed) = end_date {
                if date > ed {
                    continue;
                }
            }
            if let Some(close) = closes.get(i) {
                data.push((date, close));
            }
        }

        if data.is_empty() {
            missing.push(symbol.clone());
            continue;
        }

        series_by_symbol.push((symbol.clone(), data));
    }

    Ok((series_by_symbol, missing))
}

/// Load a close-price panel from bronze parquet as a `BTreeMap`.
///
/// This is the primary price-loading interface for all strategies.
/// Returns the same `(BTreeMap<symbol, series>, missing)` shape that the
/// strategy code expects.
pub fn load_price_panel(
    symbols: &[String],
    warehouse: Option<&Path>,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<(
    std::collections::BTreeMap<String, Vec<(NaiveDate, f64)>>,
    Vec<String>,
)> {
    let (frame, missing) = load_close_frame(symbols, warehouse, start_date, end_date)?;
    let map: std::collections::BTreeMap<String, Vec<(NaiveDate, f64)>> =
        frame.into_iter().collect();
    Ok((map, missing))
}

/// Load VIX OHLCV from local warehouse parquet at `asset_class=volatility/symbol=VIX`.
///
/// Same parquet schema as equities (trade_date, open, high, low, close, volume).
/// No HTTP download — pure local data.
pub fn load_vix_ohlcv(warehouse: Option<&std::path::Path>) -> Result<Vec<OhlcvRow>> {
    load_volatility_index_ohlcv("VIX", warehouse)
}

/// Load any volatility index (VIX, VVIX, VIX3M, etc.) from local warehouse parquet.
pub fn load_volatility_index_ohlcv(
    symbol: &str,
    warehouse: Option<&std::path::Path>,
) -> Result<Vec<OhlcvRow>> {
    let root = match warehouse {
        Some(w) => w.to_path_buf(),
        None => warehouse_root()?,
    };
    let data_file = root
        .join("data-lake")
        .join("bronze")
        .join("asset_class=volatility")
        .join(format!("symbol={symbol}"))
        .join("data.parquet");

    if !data_file.exists() {
        bail!(
            "{symbol} parquet not found: {}. Ensure data is in the warehouse at asset_class=volatility/symbol={symbol}/",
            data_file.display()
        );
    }

    let df = LazyFrame::scan_parquet(&data_file, Default::default())?
        .select([
            col("trade_date"),
            col("open"),
            col("high"),
            col("low"),
            col("close"),
            col("volume"),
        ])
        .sort(["trade_date"], Default::default())
        .collect()
        .with_context(|| format!("reading {symbol} parquet from volatility asset class"))?;

    dataframe_to_ohlcv(&df)
}

const VIX_URL: &str = "https://cdn.cboe.com/api/global/us_indices/daily_prices/VIX_History.csv";
const STALE_SECONDS: u64 = 86400;

/// VIX data row.
#[derive(Debug, Clone)]
pub struct VixRow {
    pub trade_date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Download/cache CBOE VIX CSV and return parsed rows.
pub fn load_vix_from_cboe(cache_path: Option<&Path>) -> Result<Vec<VixRow>> {
    let cache = match cache_path {
        Some(p) => p.to_path_buf(),
        None => {
            let root = warehouse_root()?;
            root.join("data-lake")
                .join("bronze")
                .join("external")
                .join("vix_cboe_history.csv")
        }
    };

    let need_download = if cache.exists() {
        let metadata = std::fs::metadata(&cache)?;
        let modified = metadata.modified()?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs();
        age >= STALE_SECONDS
    } else {
        true
    };

    if need_download {
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let resp =
            reqwest::blocking::get(VIX_URL).with_context(|| "downloading VIX data from CBOE")?;
        let text = resp.text()?;
        std::fs::write(&cache, &text)?;
    }

    parse_vix_csv(&cache)
}

fn parse_vix_csv(path: &Path) -> Result<Vec<VixRow>> {
    let content = std::fs::read_to_string(path)?;
    let mut rows = Vec::new();

    let mut lines = content.lines();
    let header = lines.next().context("empty VIX CSV")?;
    let headers: Vec<String> = header.split(',').map(|s| s.trim().to_lowercase()).collect();
    let date_idx = headers
        .iter()
        .position(|h| h == "date")
        .context("no date column in VIX CSV")?;
    let open_idx = headers
        .iter()
        .position(|h| h == "open")
        .context("no open column")?;
    let high_idx = headers
        .iter()
        .position(|h| h == "high")
        .context("no high column")?;
    let low_idx = headers
        .iter()
        .position(|h| h == "low")
        .context("no low column")?;
    let close_idx = headers
        .iter()
        .position(|h| h == "close")
        .context("no close column")?;

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() <= close_idx {
            continue;
        }

        let date_str = fields[date_idx].trim();
        // Try multiple date formats
        let trade_date = NaiveDate::parse_from_str(date_str, "%m/%d/%Y")
            .or_else(|_| NaiveDate::parse_from_str(date_str, "%Y-%m-%d"))
            .with_context(|| format!("parsing VIX date '{date_str}'"))?;

        let open: f64 = fields[open_idx].trim().parse().unwrap_or(f64::NAN);
        let high: f64 = fields[high_idx].trim().parse().unwrap_or(f64::NAN);
        let low: f64 = fields[low_idx].trim().parse().unwrap_or(f64::NAN);
        let close: f64 = fields[close_idx].trim().parse().unwrap_or(f64::NAN);

        rows.push(VixRow {
            trade_date,
            open,
            high,
            low,
            close,
        });
    }

    rows.sort_by_key(|r| r.trade_date);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_vix_csv() {
        let tmp = tempfile::tempdir().unwrap();
        let csv_path = tmp.path().join("vix.csv");
        fs::write(
            &csv_path,
            "DATE,OPEN,HIGH,LOW,CLOSE\n01/02/2020,13.00,14.00,12.00,13.50\n01/03/2020,13.50,15.00,13.00,14.00\n",
        )
        .unwrap();

        let rows = parse_vix_csv(&csv_path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].trade_date,
            NaiveDate::from_ymd_opt(2020, 1, 2).unwrap()
        );
        assert_eq!(rows[0].close, 13.50);
        assert_eq!(
            rows[1].trade_date,
            NaiveDate::from_ymd_opt(2020, 1, 3).unwrap()
        );
    }

    #[test]
    fn test_parse_vix_csv_uses_cache() {
        // Write a CSV file, parse it, verify data
        let tmp = tempfile::tempdir().unwrap();
        let csv_path = tmp.path().join("cached_vix.csv");
        fs::write(
            &csv_path,
            "DATE,OPEN,HIGH,LOW,CLOSE\n01/02/2020,13.50,14.00,13.00,13.78\n",
        )
        .unwrap();

        let rows = parse_vix_csv(&csv_path).unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].close - 13.78).abs() < 1e-6);
    }

    #[test]
    fn test_parse_vix_csv_iso_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let csv_path = tmp.path().join("vix_iso.csv");
        fs::write(
            &csv_path,
            "DATE,OPEN,HIGH,LOW,CLOSE\n2020-01-02,13.50,14.00,13.00,13.78\n",
        )
        .unwrap();

        let rows = parse_vix_csv(&csv_path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].trade_date,
            NaiveDate::from_ymd_opt(2020, 1, 2).unwrap()
        );
    }

    #[test]
    fn test_load_ticker_ohlcv_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_ticker_ohlcv("FAKE", Some(tmp.path()), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_close_frame_missing_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let warehouse = tmp.path().join("warehouse");
        let bronze = warehouse
            .join("data-lake")
            .join("bronze")
            .join("asset_class=equity");
        fs::create_dir_all(&bronze).unwrap();

        let symbols = vec!["NOPE".to_string()];
        let result = load_close_frame(&symbols, Some(warehouse.as_path()), None, None);
        let (data, missing) = result.unwrap();
        assert!(data.is_empty());
        assert_eq!(missing, vec!["NOPE"]);
    }

    #[test]
    fn test_load_vix_ohlcv_missing_parquet() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_vix_ohlcv(Some(tmp.path()));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("VIX parquet not found"), "got: {err_msg}");
    }
}
