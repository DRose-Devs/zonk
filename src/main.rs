use std::time::Instant;

use anyhow::Result;
use clap::Parser;

use zonk::cli::{Cli, Command, OutputFormat, StrategyCommand};

fn run_strategy(strategy: StrategyCommand, fmt: OutputFormat) -> Result<()> {
    match strategy {
        StrategyCommand::OvernightDrift(args) => {
            zonk::strategies::overnight_drift::run(&args, fmt)?;
        }
        StrategyCommand::IntradayDrift(args) => {
            zonk::strategies::intraday_drift::run(&args, fmt)?;
        }
        StrategyCommand::BreadthWashout(args) => {
            zonk::strategies::breadth_washout::run(&args, fmt)?;
        }
        StrategyCommand::BreadthMa(args) => {
            zonk::strategies::breadth_ma::run(&args, fmt)?;
        }
        StrategyCommand::BreadthDualMa(args) => {
            zonk::strategies::breadth_dual_ma::run(&args, fmt)?;
        }
        StrategyCommand::Ndx100SmaBreadth(args) => {
            zonk::strategies::ndx100_sma_breadth::run(&args, fmt)?;
        }
        StrategyCommand::Ndx100BreadthWashout(args) => {
            zonk::strategies::ndx100_breadth_washout::run(&args, fmt)?;
        }
        StrategyCommand::PaperResearch(args) => {
            zonk::strategies::paper_research::run(&args, fmt)?;
        }
    }

    Ok(())
}

fn emit_elapsed(fmt: OutputFormat, t0: Instant) {
    if fmt != OutputFormat::Text {
        return;
    }

    let elapsed = t0.elapsed();
    let secs = elapsed.as_secs_f64();
    if secs >= 60.0 {
        let mins = (secs / 60.0).floor() as u64;
        let rem = secs - (mins as f64 * 60.0);
        eprintln!("\n⏱  {}m {:.2}s", mins, rem);
    } else {
        eprintln!("\n⏱  {:.2}s", secs);
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let fmt = cli.output;

    // Only init tracing for text output (avoid polluting JSON stdout)
    if fmt == OutputFormat::Text {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::INFO.into()),
            )
            .init();
    }

    let t0 = Instant::now();

    match cli.command {
        Command::Run { strategy } => {
            run_strategy(strategy, fmt)?;
            emit_elapsed(fmt, t0);
        }
        Command::PaperResearch(args) => {
            run_strategy(StrategyCommand::PaperResearch(args), fmt)?;
            emit_elapsed(fmt, t0);
        }
        Command::ListStrategies => {
            zonk::cli::list_strategies();
        }
        Command::ListPresets => {
            zonk::cli::list_presets();
        }
    }

    Ok(())
}
