//! Offline validation of golf models against measured targets.

mod corpus;
mod report;
mod score;
mod target;

pub use corpus::{Corpus, Dataset, Fetch, Ledger, LoadError, SkippedDataset, Verdict};
pub use report::{Model, QuantityResult, Report, Status, TargetResult};
pub use score::{
    Counts, QuantityStats, ResidualStats, RowChange, RowKey, RowScores, Scorecard, ScorecardDiff,
    ScorecardRow, SubmodelChange, SubmodelScore,
};
pub use target::{
    Check, Conditions, Earlier, EarlierVia, Fit, Green, Ground, Kind, Quantities, Quantity, Role,
    Submodel, Target, TargetFile, Tolerance, ToleranceKind,
};
