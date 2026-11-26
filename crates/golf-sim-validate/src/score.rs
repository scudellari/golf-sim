use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::{
    Report, Role, SkippedDataset, Status, Submodel, ToleranceKind, Verdict,
    report::{display_skipped, display_value},
};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct Scorecard {
    pub rows: Vec<ScorecardRow>,
    pub submodels: Vec<SubmodelScore>,
    pub skipped_datasets: Vec<SkippedDataset>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RowKey {
    pub source: String,
    pub submodel: Submodel,
    pub role: Role,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScorecardRow {
    #[serde(flatten)]
    pub key: RowKey,
    #[serde(flatten)]
    pub scores: RowScores,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RowScores {
    pub datasets: BTreeMap<String, Verdict>,
    pub tolerance_kinds: BTreeSet<ToleranceKind>,
    pub counts: Counts,
    pub quantities: BTreeMap<String, QuantityStats>,
    /// The mean of the normalized mean absolute residuals across quantities.
    pub score: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Counts {
    pub targets: usize,
    pub passes: usize,
    pub failures: usize,
    pub reported_only: usize,
    pub not_run: usize,
    /// Only known target IDs can be counted in a row. Absent datasets are
    /// listed on the scorecard with no assumed target counts.
    pub skipped: usize,
}

impl Counts {
    fn record(&mut self, status: Status) {
        self.targets += 1;
        match status {
            Status::Passed => self.passes += 1,
            Status::Failed => self.failures += 1,
            Status::ReportedOnly => self.reported_only += 1,
            Status::NotRun => self.not_run += 1,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct QuantityStats {
    pub residual: Option<ResidualStats>,
    /// Residual divided by the source tolerance half-width, where positive.
    pub share_of_tolerance: Option<ResidualStats>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResidualStats {
    pub count: usize,
    pub mean_absolute: f64,
    pub rms: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SubmodelScore {
    pub submodel: Submodel,
    pub role: Role,
    /// The equal-weight mean of the available scores for this submodel and role.
    pub score: Option<f64>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct ScorecardDiff {
    pub rows: Vec<RowChange>,
    pub submodels: Vec<SubmodelChange>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct RowChange {
    #[serde(flatten)]
    pub key: RowKey,
    pub before: Option<RowScores>,
    pub after: Option<RowScores>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct SubmodelChange {
    pub submodel: Submodel,
    pub role: Role,
    pub before: Option<SubmodelScore>,
    pub after: Option<SubmodelScore>,
}

impl Report {
    pub fn scorecard(&self) -> Scorecard {
        let mut rows = BTreeMap::<_, RowAccumulator>::new();
        for target in &self.targets {
            let mut roles = BTreeMap::new();
            for (name, quantity) in &target.quantities {
                let row = rows
                    .entry(RowKey {
                        source: target.source.clone(),
                        submodel: target.submodel,
                        role: quantity.role,
                    })
                    .or_default();
                row.datasets.extend(target.datasets.clone());
                roles
                    .entry(quantity.role)
                    .and_modify(|status: &mut Status| *status = (*status).max(quantity.status))
                    .or_insert(quantity.status);
                if let Some(band) = quantity.source_tolerance {
                    row.tolerance_kinds.insert(band.kind());
                }
                let stats = row.quantities.entry(name.clone()).or_default();
                if let Some(residual) = quantity.residual {
                    stats.residual.add(residual.abs());
                    if let Some(band) = quantity.source_tolerance
                        && band.half_width() > 0.0
                    {
                        let share = residual.abs() / band.half_width();
                        if share.is_finite() {
                            stats.share.add(share);
                        }
                    }
                }
            }
            for (role, status) in roles {
                rows.get_mut(&RowKey {
                    source: target.source.clone(),
                    submodel: target.submodel,
                    role,
                })
                .expect("a quantity created this row")
                .counts
                .record(status);
            }
        }

        let mut submodels = BTreeMap::<_, Accumulator>::new();
        let rows = rows
            .into_iter()
            .map(|(key, row)| {
                let scores = row.finish();
                let submodel = submodels.entry((key.submodel, key.role)).or_default();
                if let Some(score) = scores.score {
                    submodel.add(score);
                }
                ScorecardRow { key, scores }
            })
            .collect();
        let submodels = submodels
            .into_iter()
            .map(|((submodel, role), stats)| SubmodelScore {
                submodel,
                role,
                score: stats.mean(),
            })
            .collect();

        Scorecard {
            rows,
            submodels,
            skipped_datasets: self.skipped_datasets.clone(),
        }
    }
}

impl Scorecard {
    /// List changed rows and submodel scores in key order, including arrivals and removals.
    pub fn diff(before: &Self, after: &Self) -> ScorecardDiff {
        let rows = changed(
            before.rows.iter().map(|row| (&row.key, &row.scores)),
            after.rows.iter().map(|row| (&row.key, &row.scores)),
        )
        .map(|(key, before, after)| RowChange {
            key: key.clone(),
            before: before.cloned(),
            after: after.cloned(),
        })
        .collect();
        let submodels = changed(
            before
                .submodels
                .iter()
                .map(|row| ((row.submodel, row.role), row)),
            after
                .submodels
                .iter()
                .map(|row| ((row.submodel, row.role), row)),
        )
        .map(|((submodel, role), before, after)| SubmodelChange {
            submodel,
            role,
            before: before.cloned(),
            after: after.cloned(),
        })
        .collect();
        ScorecardDiff { rows, submodels }
    }
}

fn changed<K: Copy + Ord, V: PartialEq>(
    before: impl Iterator<Item = (K, V)>,
    after: impl Iterator<Item = (K, V)>,
) -> impl Iterator<Item = (K, Option<V>, Option<V>)> {
    let mut pairs = BTreeMap::<_, (Option<V>, Option<V>)>::new();
    for (key, value) in before {
        pairs.entry(key).or_default().0 = Some(value);
    }
    for (key, value) in after {
        pairs.entry(key).or_default().1 = Some(value);
    }
    pairs
        .into_iter()
        .filter_map(|(key, (before, after))| (before != after).then_some((key, before, after)))
}

#[derive(Default)]
struct RowAccumulator {
    datasets: BTreeMap<String, Verdict>,
    tolerance_kinds: BTreeSet<ToleranceKind>,
    counts: Counts,
    quantities: BTreeMap<String, QuantityAccumulator>,
}

impl RowAccumulator {
    fn finish(self) -> RowScores {
        let quantities: BTreeMap<_, _> = self
            .quantities
            .into_iter()
            .map(|(name, acc)| {
                (
                    name,
                    QuantityStats {
                        residual: acc.residual.finish(),
                        share_of_tolerance: acc.share.finish(),
                    },
                )
            })
            .collect();
        let mut scores = Accumulator::default();
        for share in quantities
            .values()
            .filter_map(|quantity| quantity.share_of_tolerance)
        {
            scores.add(share.mean_absolute);
        }
        RowScores {
            datasets: self.datasets,
            tolerance_kinds: self.tolerance_kinds,
            counts: self.counts,
            quantities,
            score: scores.mean(),
        }
    }
}

#[derive(Default)]
struct QuantityAccumulator {
    residual: Accumulator,
    share: Accumulator,
}

#[derive(Default)]
struct Accumulator {
    count: usize,
    scale: f64,
    sum: f64,
    squares: f64,
}

impl Accumulator {
    fn add(&mut self, value: f64) {
        self.count += 1;
        if value == 0.0 {
            return;
        }
        if value > self.scale {
            let ratio = self.scale / value;
            self.sum *= ratio;
            self.squares *= ratio * ratio;
            self.scale = value;
        }
        let value = value / self.scale;
        self.sum += value;
        self.squares += value * value;
    }

    fn mean(&self) -> Option<f64> {
        (self.count != 0).then(|| self.scale * (self.sum / self.count as f64))
    }

    fn finish(&self) -> Option<ResidualStats> {
        Some(ResidualStats {
            count: self.count,
            mean_absolute: self.mean()?,
            rms: self.scale * root_of_mean_square(self.squares / self.count as f64),
        })
    }
}

fn root_of_mean_square(value: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    // Scaling keeps the mean square normal. Round its significand using integer
    // arithmetic so reporting needs no platform math or core-crate dependency.
    const FRACTION: u64 = (1 << 52) - 1;
    let bits = value.to_bits();
    let exponent = (bits >> 52) as i32 - 1023;
    let significand = (1 << 52) | (bits & FRACTION);
    let radicand = u128::from(significand) << (52 + exponent.rem_euclid(2));
    let root = radicand.isqrt();
    let rounded = root + u128::from(radicand - root * root > root);
    let exponent_bits = ((exponent.div_euclid(2) + 1023) as u64) << 52;
    f64::from_bits(exponent_bits + (rounded as u64 - (1 << 52)))
}

const ROW_HEADER: &str = "source\tsubmodel\trole\tdatasets\ttolerances\ttargets\tpasses\tfailures\treported only\tnot run\tskipped\tscore";
const STATS_HEADER: &str = "source\tsubmodel\trole\tquantity\tn\tMAE\tRMS\tn with tolerance\tMAE / tolerance\tRMS / tolerance";

impl fmt::Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\t{}\t{}", self.source, self.submodel, self.role)
    }
}

fn row_text(f: &mut fmt::Formatter<'_>, key: &RowKey, row: &RowScores) -> fmt::Result {
    let datasets = row
        .datasets
        .iter()
        .map(|(name, verdict)| format!("{name}={verdict}"))
        .collect::<Vec<_>>()
        .join(", ");
    let tolerances = row
        .tolerance_kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let c = &row.counts;
    writeln!(
        f,
        "{key}\t{datasets}\t{tolerances}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        c.targets,
        c.passes,
        c.failures,
        c.reported_only,
        c.not_run,
        c.skipped,
        display_value(row.score)
    )
}

fn stats_text(f: &mut fmt::Formatter<'_>, key: impl fmt::Display, row: &RowScores) -> fmt::Result {
    for (name, stats) in &row.quantities {
        writeln!(
            f,
            "{key}\t{name}\t{}\t{}",
            stats_columns(stats.residual),
            stats_columns(stats.share_of_tolerance)
        )?;
    }
    Ok(())
}

fn stats_columns(stats: Option<ResidualStats>) -> String {
    stats.map_or_else(
        || "0\t-\t-".into(),
        |stats| format!("{}\t{}\t{}", stats.count, stats.mean_absolute, stats.rms),
    )
}

impl fmt::Display for Scorecard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{ROW_HEADER}")?;
        for row in &self.rows {
            row_text(f, &row.key, &row.scores)?;
        }
        writeln!(f, "\n{STATS_HEADER}")?;
        for row in &self.rows {
            stats_text(f, &row.key, &row.scores)?;
        }
        writeln!(f, "\nsubmodel\trole\tscore")?;
        for entry in &self.submodels {
            writeln!(
                f,
                "{}\t{}\t{}",
                entry.submodel,
                entry.role,
                display_value(entry.score)
            )?;
        }
        display_skipped(f, &self.skipped_datasets)
    }
}

impl fmt::Display for ScorecardDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "version\t{ROW_HEADER}")?;
        for change in &self.rows {
            for (version, row) in [("before", &change.before), ("after", &change.after)] {
                if let Some(row) = row {
                    write!(f, "{version}\t")?;
                    row_text(f, &change.key, row)?;
                }
            }
        }
        writeln!(f, "\nversion\t{STATS_HEADER}")?;
        for change in &self.rows {
            for (version, row) in [("before", &change.before), ("after", &change.after)] {
                if let Some(row) = row {
                    stats_text(f, format_args!("{version}\t{}", change.key), row)?;
                }
            }
        }
        writeln!(f, "\nsubmodel\trole\tbefore\tafter")?;
        for entry in &self.submodels {
            writeln!(
                f,
                "{}\t{}\t{}\t{}",
                entry.submodel,
                entry.role,
                display_value(entry.before.as_ref().and_then(|row| row.score)),
                display_value(entry.after.as_ref().and_then(|row| row.score))
            )?;
        }
        Ok(())
    }
}
