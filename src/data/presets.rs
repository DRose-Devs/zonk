/// Preset loading and validation.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::presets_dir;

#[derive(Deserialize)]
struct PresetPayload {
    name: Option<String>,
    tickers: Option<Vec<String>>,
}

/// Load a preset by name (looks in presets/) or by path.
///
/// Returns `(preset_name, tickers)`.
pub fn load_preset(name_or_path: &str) -> Result<(String, Vec<String>)> {
    let path = if Path::new(name_or_path)
        .extension()
        .is_some_and(|ext| !ext.is_empty())
    {
        PathBuf::from(name_or_path)
    } else {
        presets_dir().join(format!("{name_or_path}.json"))
    };

    if !path.exists() {
        bail!("Preset not found: {}", path.display());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading preset {}", path.display()))?;
    let payload: PresetPayload = serde_json::from_str(&content)
        .with_context(|| format!("parsing preset {}", path.display()))?;

    let name = payload
        .name
        .unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().to_string());

    let tickers = payload.tickers;
    match tickers {
        Some(ref t) if !t.is_empty() => {}
        _ => bail!(
            "Preset {} does not contain a non-empty ticker list",
            path.display()
        ),
    }
    let tickers = tickers.unwrap();

    let mut seen = HashSet::new();
    let unique: Vec<String> = tickers
        .into_iter()
        .map(|t| t.to_uppercase())
        .filter(|t| seen.insert(t.clone()))
        .collect();

    Ok((name, unique))
}

/// Discover available preset names in the presets directory.
pub fn list_presets() -> Vec<String> {
    let root = presets_dir();
    if !root.exists() {
        return Vec::new();
    }
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_preset_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.json");
        fs::write(
            &path,
            r#"{"name": "Test", "tickers": ["AAPL", "msft", "aapl"]}"#,
        )
        .unwrap();

        let (name, tickers) = load_preset(path.to_str().unwrap()).unwrap();
        assert_eq!(name, "Test");
        assert_eq!(tickers, vec!["AAPL", "MSFT"]);
    }

    #[test]
    fn test_load_preset_missing() {
        let result = load_preset("/tmp/nonexistent-preset-test.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_preset_empty_tickers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.json");
        fs::write(&path, r#"{"tickers": []}"#).unwrap();
        let result = load_preset(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_preset_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.json");
        fs::write(&path, "{not valid json").unwrap();
        let result = load_preset(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_list_presets_returns_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("b.json"), "{}").unwrap();
        fs::write(tmp.path().join("a.json"), "{}").unwrap();
        fs::write(tmp.path().join("not-json.txt"), "").unwrap();

        // We can't easily override presets_dir, but we can test the listing logic directly
        let root = tmp.path();
        let mut names: Vec<String> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn test_load_preset_deduplicates_case() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dups.json");
        fs::write(&path, r#"{"tickers": ["spy", "SPY", "qqq"]}"#).unwrap();
        let (_, tickers) = load_preset(path.to_str().unwrap()).unwrap();
        assert_eq!(tickers, vec!["SPY", "QQQ"]);
    }

    #[test]
    fn test_load_preset_uses_filename_as_name() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("my-preset.json");
        fs::write(&path, r#"{"tickers": ["AAPL"]}"#).unwrap();
        let (name, _) = load_preset(path.to_str().unwrap()).unwrap();
        assert_eq!(name, "my-preset");
    }
}
