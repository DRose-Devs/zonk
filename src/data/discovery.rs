/// Symbol discovery from the bronze parquet layer.
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use polars::prelude::*;

use crate::config::bronze_equity_dir;

/// Return all symbols currently stored in the canonical bronze layer.
///
/// Scans for `symbol=<TICKER>` directories containing `data.parquet`.
pub fn discover_symbols(bronze_dir: Option<&Path>) -> Result<Vec<String>> {
    let root = match bronze_dir {
        Some(p) => p.to_path_buf(),
        None => bronze_equity_dir(None)?,
    };

    if !root.exists() {
        bail!("Bronze equity directory not found: {}", root.display());
    }

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_dir() && name.starts_with("symbol=") {
            let ticker = name.splitn(2, '=').nth(1).unwrap_or("").to_string();
            if entry.path().join("data.parquet").exists() {
                entries.push((ticker, entry.path()));
            }
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries.into_iter().map(|(ticker, _)| ticker).collect())
}

/// Discover all warehouse symbols and filter by minimum parquet row count.
///
/// Returns only symbols whose `data.parquet` has at least `min_rows` rows,
/// sorted alphabetically. Uses parquet metadata for fast row-count reads.
pub fn discover_viable_symbols(bronze_dir: Option<&Path>, min_rows: usize) -> Result<Vec<String>> {
    let root = match bronze_dir {
        Some(p) => p.to_path_buf(),
        None => bronze_equity_dir(None)?,
    };

    if !root.exists() {
        bail!("Bronze equity directory not found: {}", root.display());
    }

    let mut viable = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type()?.is_dir() || !name.starts_with("symbol=") {
            continue;
        }
        let ticker = name.splitn(2, '=').nth(1).unwrap_or("").to_string();
        let parquet_path = entry.path().join("data.parquet");
        if !parquet_path.exists() {
            continue;
        }

        // Read row count via polars LazyFrame metadata scan
        let row_count = LazyFrame::scan_parquet(&parquet_path, Default::default())
            .and_then(|lf| lf.select([len()]).collect())
            .ok()
            .and_then(|df| df.column("len").ok().cloned())
            .and_then(|col| col.u32().ok().and_then(|ca| ca.get(0)).map(|v| v as usize));

        if let Some(count) = row_count {
            if count >= min_rows {
                viable.push(ticker);
            }
        }
    }

    viable.sort();
    Ok(viable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_discover_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        // Create symbol directories
        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        fs::write(spy_dir.join("data.parquet"), b"fake").unwrap();

        let aapl_dir = bronze.join("symbol=AAPL");
        fs::create_dir_all(&aapl_dir).unwrap();
        fs::write(aapl_dir.join("data.parquet"), b"fake").unwrap();

        // Dir without parquet — should be skipped
        let no_data = bronze.join("symbol=NOPE");
        fs::create_dir_all(&no_data).unwrap();

        // Non-symbol dir — should be skipped
        let other = bronze.join("other_dir");
        fs::create_dir_all(&other).unwrap();

        let symbols = discover_symbols(Some(bronze)).unwrap();
        assert_eq!(symbols, vec!["AAPL", "SPY"]);
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let result = discover_symbols(Some(Path::new("/tmp/nonexistent-bronze-test")));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_bronze_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // Empty directory, no symbol=* subdirs
        let symbols = discover_symbols(Some(tmp.path())).unwrap();
        assert!(symbols.is_empty());
    }

    /// Helper: create a minimal parquet file with `n` rows.
    fn write_test_parquet(path: &std::path::Path, n: usize) {
        use polars::prelude::*;
        let dates: Vec<i32> = (0..n as i32).collect();
        let closes: Vec<f64> = (0..n).map(|i| 100.0 + i as f64).collect();
        let df = DataFrame::new(vec![
            Column::new("trade_date".into(), &dates),
            Column::new("close".into(), &closes),
        ])
        .unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        ParquetWriter::new(&mut file)
            .finish(&mut df.clone())
            .unwrap();
    }

    #[test]
    fn test_discover_viable_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        // SPY: 500 rows — should pass threshold of 100
        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        write_test_parquet(&spy_dir.join("data.parquet"), 500);

        // AAPL: 50 rows — should fail threshold of 100
        let aapl_dir = bronze.join("symbol=AAPL");
        fs::create_dir_all(&aapl_dir).unwrap();
        write_test_parquet(&aapl_dir.join("data.parquet"), 50);

        // MSFT: 200 rows — should pass threshold of 100
        let msft_dir = bronze.join("symbol=MSFT");
        fs::create_dir_all(&msft_dir).unwrap();
        write_test_parquet(&msft_dir.join("data.parquet"), 200);

        let viable = discover_viable_symbols(Some(bronze), 100).unwrap();
        assert_eq!(viable, vec!["MSFT", "SPY"]);
    }

    #[test]
    fn test_discover_viable_symbols_empty_on_high_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        write_test_parquet(&spy_dir.join("data.parquet"), 100);

        let viable = discover_viable_symbols(Some(bronze), 999_999).unwrap();
        assert!(viable.is_empty());
    }

    #[test]
    fn test_skips_dirs_without_parquet() {
        let tmp = tempfile::tempdir().unwrap();
        let bronze = tmp.path();

        // Dir with parquet
        let spy_dir = bronze.join("symbol=SPY");
        fs::create_dir_all(&spy_dir).unwrap();
        fs::write(spy_dir.join("data.parquet"), b"fake").unwrap();

        // Dir without parquet — should be skipped
        let empty_dir = bronze.join("symbol=EMPTY");
        fs::create_dir_all(&empty_dir).unwrap();

        let symbols = discover_symbols(Some(bronze)).unwrap();
        assert_eq!(symbols, vec!["SPY"]);
    }
}
