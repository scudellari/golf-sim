//! Log configuration for the golf program.

use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cmd::Outcome;

pub(crate) fn with_log(
    path: Option<&Path>,
    level: LevelFilter,
    run: impl FnOnce() -> Outcome,
) -> Outcome {
    let Some(path) = path else {
        return run();
    };

    let file = File::create(path)
        .map_err(|error| format!("cannot open log {}: {error}", path.display()))?;
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_max_level(level)
        .with_writer(Mutex::new(file))
        .finish();
    let _guard = subscriber.set_default();
    run()
}
