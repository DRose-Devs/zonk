/// Centralized configuration: warehouse path, output root, presets dir.
///
/// Resolution order: ZONK_WAREHOUSE_PATH env var -> .env file -> ~/market-warehouse
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

fn default_warehouse() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join("market-warehouse")
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_dotenv_value(key: &str) -> Option<String> {
    let env_file = project_root().join(".env");
    if !env_file.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&env_file).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || !line.contains('=') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                let v = v.trim().trim_matches('\'').trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Resolved warehouse path. Fails fast if the path is invalid.
pub fn warehouse_root() -> Result<PathBuf> {
    let env_val = std::env::var("ZONK_WAREHOUSE_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| load_dotenv_value("ZONK_WAREHOUSE_PATH"));

    let root = match env_val {
        Some(val) => PathBuf::from(val),
        None => default_warehouse(),
    };

    if !root.exists() {
        bail!("Warehouse not found: {}", root.display());
    }
    let bronze = root.join("data-lake").join("bronze");
    if !bronze.exists() {
        bail!(
            "Warehouse missing expected data-lake/bronze/ structure: {}",
            root.display()
        );
    }
    Ok(root)
}

/// Bronze parquet root for equities.
pub fn bronze_equity_dir(warehouse: Option<&Path>) -> Result<PathBuf> {
    let root = match warehouse {
        Some(p) => p.to_path_buf(),
        None => warehouse_root()?,
    };
    Ok(root
        .join("data-lake")
        .join("bronze")
        .join("asset_class=equity"))
}

/// Output root for generated artifacts.
pub fn output_dir() -> PathBuf {
    project_root().join("output")
}

/// Presets root directory.
pub fn presets_dir() -> PathBuf {
    project_root().join("presets")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_warehouse_is_home_based() {
        let wh = default_warehouse();
        assert!(wh.to_string_lossy().contains("market-warehouse"));
    }

    #[test]
    fn test_presets_dir_exists() {
        // Just ensure the function doesn't panic
        let _ = presets_dir();
    }

    #[test]
    fn test_output_dir_exists() {
        let _ = output_dir();
    }

    #[test]
    fn test_warehouse_root_with_env() {
        // Set env to a nonexistent path — should fail
        unsafe {
            std::env::set_var("ZONK_WAREHOUSE_PATH", "/tmp/nonexistent-warehouse-test");
        }
        let result = warehouse_root();
        assert!(result.is_err());
        unsafe {
            std::env::remove_var("ZONK_WAREHOUSE_PATH");
        }
    }

    #[test]
    fn test_missing_bronze_detected() {
        // Verify that bronze_equity_dir constructs the expected path
        // (bronze validation is in warehouse_root which needs env vars;
        // we test the path construction without env var mutation)
        let tmp = tempfile::tempdir().unwrap();
        let warehouse = tmp.path().join("bad-warehouse");
        std::fs::create_dir_all(&warehouse).unwrap();
        // bronze_equity_dir doesn't validate existence, just builds path
        let result = bronze_equity_dir(Some(warehouse.as_path())).unwrap();
        assert!(result.to_string_lossy().contains("asset_class=equity"));
        assert!(!result.exists());
    }

    #[test]
    fn test_bronze_equity_dir_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let warehouse = tmp.path().join("wh");
        let equity_dir = warehouse
            .join("data-lake")
            .join("bronze")
            .join("asset_class=equity");
        std::fs::create_dir_all(&equity_dir).unwrap();
        let result = bronze_equity_dir(Some(warehouse.as_path())).unwrap();
        assert_eq!(result, equity_dir);
    }

    #[test]
    fn test_output_dir_name() {
        let result = output_dir();
        assert_eq!(result.file_name().unwrap().to_str().unwrap(), "output");
    }

    #[test]
    fn test_presets_dir_name() {
        let result = presets_dir();
        assert_eq!(result.file_name().unwrap().to_str().unwrap(), "presets");
    }
}
