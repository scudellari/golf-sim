use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Print the lie table.
    LieTable(LieTableArgs),
    /// Print the fat-shot table.
    FatTable(FatTableArgs),
    /// Sweep the swing settings.
    SweepSwing(SweepSwingArgs),
    /// Print the tee-height table.
    TeeTable(TeeTableArgs),
}

#[derive(Args)]
pub struct LieTableArgs {}

#[derive(Args)]
pub struct FatTableArgs {}

#[derive(Args)]
pub struct SweepSwingArgs {}

#[derive(Args)]
pub struct TeeTableArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::LieTable(_) => not_built("lie-table"),
        Command::FatTable(_) => not_built("fat-table"),
        Command::SweepSwing(_) => not_built("sweep-swing"),
        Command::TeeTable(_) => not_built("tee-table"),
    }
}
