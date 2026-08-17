/// Unified CLI entrypoint using clap derive.
///
/// Usage:
///     zonk run overnight-drift [OPTIONS]
///     zonk run intraday-drift [OPTIONS]
///     zonk paper-research [OPTIONS]
///     zonk run breadth-washout [OPTIONS]
///     zonk run ndx100-sma-breadth [OPTIONS]
///     zonk list-strategies
///     zonk list-presets
use clap::{Parser, Subcommand, ValueEnum};

use crate::strategies::breadth_dual_ma::BreadthDualMaArgs;
use crate::strategies::breadth_ma::BreadthMaArgs;
use crate::strategies::breadth_washout::BreadthWashoutArgs;
use crate::strategies::intraday_drift::IntradayDriftArgs;
use crate::strategies::ndx100_breadth_washout::Ndx100BreadthWashoutArgs;
use crate::strategies::ndx100_sma_breadth::Ndx100SmaBreadthArgs;
use crate::strategies::overnight_drift::OvernightDriftArgs;
use crate::strategies::paper_research::PaperResearchArgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    Md,
}

#[derive(Parser)]
#[command(
    name = "zonk",
    about = "Quantitative strategy research and backtesting"
)]
pub struct Cli {
    /// Output format: text (default), json (structured), or md (markdown)
    #[arg(long, default_value = "text", global = true)]
    pub output: OutputFormat,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a backtesting strategy
    Run {
        #[command(subcommand)]
        strategy: StrategyCommand,
    },
    /// Compatibility shortcut for `run paper-research`
    PaperResearch(PaperResearchArgs),
    /// List available strategies
    ListStrategies,
    /// List available presets
    ListPresets,
}

#[derive(Subcommand)]
pub enum StrategyCommand {
    /// Buy close, sell next open; optional VIX filter
    OvernightDrift(OvernightDriftArgs),
    /// Buy open, sell close same day; optional short mode
    IntradayDrift(IntradayDriftArgs),
    /// Generic breadth signal across any universe
    BreadthWashout(BreadthWashoutArgs),
    /// Single MA breadth: % below/above N-day MA (default 50-day)
    BreadthMa(BreadthMaArgs),
    /// Dual MA breadth: close < short MA AND close > long MA
    BreadthDualMa(BreadthDualMaArgs),
    /// NDX-100 SMA breadth analysis + forward returns
    Ndx100SmaBreadth(Ndx100SmaBreadthArgs),
    /// NDX-100 breadth washout wrapper
    Ndx100BreadthWashout(Ndx100BreadthWashoutArgs),
    /// Paper-research strategy synthesized from paper-inspired rules
    PaperResearch(PaperResearchArgs),
}

pub fn list_strategies() {
    println!("Available strategies:");
    println!("  breadth-dual-ma");
    println!("  breadth-ma");
    println!("  breadth-washout");
    println!("  intraday-drift");
    println!("  ndx100-breadth-washout");
    println!("  ndx100-sma-breadth");
    println!("  overnight-drift");
    println!("  paper-research");
}

pub fn list_presets() {
    let presets = crate::data::presets::list_presets();
    if presets.is_empty() {
        println!("No presets found.");
        return;
    }
    println!("Available presets:");
    for name in presets {
        println!("  {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_list_strategies_prints_all() {
        // Just ensure list_strategies doesn't panic
        list_strategies();
    }

    #[test]
    fn test_parse_run_overnight_drift() {
        let cli = Cli::try_parse_from(&["zonk", "run", "overnight-drift", "--no-plots"]).unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::OvernightDrift(args) => {
                    assert!(args.no_plots);
                    assert!(!args.no_vix_filter);
                    assert_eq!(args.capital, 1_000_000.0);
                }
                _ => panic!("Expected OvernightDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_run_intraday_drift() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "intraday-drift",
            "--ticker",
            "QQQ",
            "--short",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::IntradayDrift(args) => {
                    assert_eq!(args.ticker, "QQQ");
                    assert!(args.short);
                }
                _ => panic!("Expected IntradayDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_list_strategies() {
        let cli = Cli::try_parse_from(&["zonk", "list-strategies"]).unwrap();
        assert!(matches!(cli.command, Command::ListStrategies));
    }

    #[test]
    fn test_parse_list_presets() {
        let cli = Cli::try_parse_from(&["zonk", "list-presets"]).unwrap();
        assert!(matches!(cli.command, Command::ListPresets));
    }

    #[test]
    fn test_no_args_fails() {
        let result = Cli::try_parse_from(&["zonk"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_command_fails() {
        let result = Cli::try_parse_from(&["zonk", "bogus"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_no_strategy_fails() {
        let result = Cli::try_parse_from(&["zonk", "run"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_breadth_washout() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "breadth-washout",
            "--signal-mode",
            "overbought",
            "--threshold",
            "70",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthWashout(args) => {
                    assert_eq!(args.signal_mode, "overbought");
                    assert_eq!(args.threshold, Some(70.0));
                }
                _ => panic!("Expected BreadthWashout"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_ndx100_sma_breadth() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "ndx100-sma-breadth",
            "--end-date",
            "2026-01-15",
            "--sessions",
            "100",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::Ndx100SmaBreadth(args) => {
                    assert_eq!(args.end_date, "2026-01-15");
                    assert_eq!(args.sessions, 100);
                }
                _ => panic!("Expected Ndx100SmaBreadth"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_overnight_drift_with_dates() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "overnight-drift",
            "--start-date",
            "2020-01-01",
            "--end-date",
            "2025-12-31",
            "--capital",
            "500000",
            "--no-vix-filter",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::OvernightDrift(args) => {
                    assert_eq!(args.start_date, Some("2020-01-01".to_string()));
                    assert_eq!(args.end_date, Some("2025-12-31".to_string()));
                    assert_eq!(args.capital, 500000.0);
                    assert!(args.no_vix_filter);
                }
                _ => panic!("Expected OvernightDrift"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_ndx100_breadth_washout() {
        let cli = Cli::try_parse_from(&["zonk", "run", "ndx100-breadth-washout"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Run {
                strategy: StrategyCommand::Ndx100BreadthWashout(_)
            }
        ));
    }

    #[test]
    fn test_parse_paper_research() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "paper-research",
            "--rule",
            "trend_pullback",
            "--asset",
            "QQQ",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::PaperResearch(args) => {
                    assert_eq!(args.asset, "QQQ");
                    assert_eq!(args.rule, "trend_pullback");
                }
                _ => panic!("Expected PaperResearch"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_top_level_paper_research_compat() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "paper-research",
            "--asset",
            "QQQ",
            "--rule",
            "vol_spread",
            "--fast-window",
            "12",
            "--slow-window",
            "50",
            "--rsi-window",
            "14",
            "--rsi-oversold",
            "30",
            "--rsi-overbought",
            "70",
            "--vol-window",
            "10",
            "--vol-cap",
            "0.15",
            "--hypothesis-id",
            "seed-24-2",
            "--output",
            "json",
        ])
        .unwrap();

        assert_eq!(cli.output, OutputFormat::Json);
        match cli.command {
            Command::PaperResearch(args) => {
                assert_eq!(args.asset, "QQQ");
                assert_eq!(args.rule, "vol_spread");
                assert_eq!(args.fast_window, 12);
                assert_eq!(args.slow_window, 50);
                assert_eq!(args.rsi_window, 14);
                assert_eq!(args.rsi_oversold, 30.0);
                assert_eq!(args.rsi_overbought, 70.0);
                assert_eq!(args.vol_window, 10);
                assert_eq!(args.vol_cap, 0.15);
                assert_eq!(args.hypothesis_id.as_deref(), Some("seed-24-2"));
            }
            _ => panic!("Expected top-level PaperResearch command"),
        }
    }

    #[test]
    fn test_output_default_is_text() {
        let cli = Cli::try_parse_from(&["zonk", "list-strategies"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Text);
    }

    #[test]
    fn test_output_json_flag() {
        let cli =
            Cli::try_parse_from(&["zonk", "--output", "json", "run", "overnight-drift"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn test_output_json_flag_after_subcommand() {
        let cli =
            Cli::try_parse_from(&["zonk", "run", "overnight-drift", "--output", "json"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn test_output_md_flag() {
        let cli =
            Cli::try_parse_from(&["zonk", "--output", "md", "run", "overnight-drift"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Md);
    }

    #[test]
    fn test_parse_breadth_ma() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "breadth-ma",
            "--short-period",
            "50",
            "--threshold",
            "80",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthMa(args) => {
                    assert_eq!(args.short_period, 50);
                    assert_eq!(args.threshold, 80.0);
                }
                _ => panic!("Expected BreadthMa"),
            },
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_parse_breadth_dual_ma() {
        let cli = Cli::try_parse_from(&[
            "zonk",
            "run",
            "breadth-dual-ma",
            "--short-period",
            "50",
            "--long-period",
            "200",
            "--threshold",
            "40",
        ])
        .unwrap();
        match cli.command {
            Command::Run { strategy } => match strategy {
                StrategyCommand::BreadthDualMa(args) => {
                    assert_eq!(args.short_period, 50);
                    assert_eq!(args.long_period, 200);
                    assert_eq!(args.threshold, 40.0);
                }
                _ => panic!("Expected BreadthDualMa"),
            },
            _ => panic!("Expected Run command"),
        }
    }
}
