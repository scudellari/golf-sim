use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Print the JSON schema of the play protocol.
    Schema(SchemaArgs),
}

#[derive(Args)]
pub struct SchemaArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Schema(_) => not_built("schema"),
    }
}
