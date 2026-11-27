use std::io::{self, Write};

use clap::Subcommand;
use schemars::{JsonSchema, schema_for};
use serde_json::json;

use super::Outcome;
use crate::protocol::{Reply, Request, VERSION};

#[derive(Subcommand)]
pub enum Command {
    /// Print the JSON schema of the play protocol.
    Schema,
}

#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Schema covers both directions of the same stream.
enum Message {
    Request(Request),
    Reply(Reply),
}

pub fn run(Command::Schema: Command) -> Outcome {
    let mut schema = schema_for!(Message);
    schema.insert("protocol".into(), json!(VERSION));
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &schema)?;
    writeln!(out)?;
    Ok(())
}
