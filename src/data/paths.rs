/// Parquet path resolution helpers for the warehouse data lake.
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::warehouse_root;

/// Return the canonical bronze parquet path for an equity ticker.
pub fn parquet_path_for_symbol(symbol: &str, warehouse: Option<&Path>) -> Result<PathBuf> {
    let root = match warehouse {
        Some(p) => p.to_path_buf(),
        None => warehouse_root()?,
    };
    Ok(root
        .join("data-lake")
        .join("bronze")
        .join("asset_class=equity")
        .join(format!("symbol={symbol}"))
        .join("data.parquet"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_path_construction() {
        let wh = Path::new("/tmp/warehouse");
        let path = parquet_path_for_symbol("SPY", Some(wh)).unwrap();
        assert_eq!(
            path,
            PathBuf::from(
                "/tmp/warehouse/data-lake/bronze/asset_class=equity/symbol=SPY/data.parquet"
            )
        );
    }
}
