/// Compatibility wrapper for the generic breadth washout strategy.
use anyhow::Result;

use crate::cli::OutputFormat;
use crate::strategies::breadth_washout::{BreadthWashoutArgs, run as breadth_washout_run};

/// CLI arguments (delegates to breadth-washout).
pub type Ndx100BreadthWashoutArgs = BreadthWashoutArgs;

pub fn run(args: &Ndx100BreadthWashoutArgs, fmt: OutputFormat) -> Result<()> {
    breadth_washout_run(args, fmt)
}
