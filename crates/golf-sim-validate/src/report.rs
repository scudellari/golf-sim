use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    Conditions, Corpus, Kind, Quantities, Quantity, Role, SkippedDataset, Submodel, Target,
    Tolerance, Verdict, corpus::Layers, target::named_enum,
};

pub trait Model {
    /// Predict the measured quantities using the target inputs and file conditions.
    /// Return `None` when this target is outside the model's scope.
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities>;

    /// Return predictions and, where needed, the sum of the magnitudes of
    /// rounded operands in the predicted quantity's units.
    fn predict_with_operand_magnitudes(
        &self,
        target: &Target,
        conditions: &Conditions,
    ) -> Option<(Quantities, BTreeMap<String, f64>)> {
        self.predict(target, conditions)
            .map(|predictions| (predictions, BTreeMap::new()))
    }
}

impl<F: Fn(&Target, &Conditions) -> Option<Quantities>> Model for F {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        self(target, conditions)
    }
}

named_enum!(Status {
    NotRun => "not-run",
    ReportedOnly => "reported-only",
    Passed => "pass",
    Failed => "fail",
});

#[derive(Debug, Deserialize, Serialize)]
pub struct Report {
    pub targets: Vec<TargetResult>,
    pub skipped_datasets: Vec<SkippedDataset>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TargetResult {
    pub id: String,
    pub source: String,
    pub club: Option<String>,
    pub shot_type: String,
    pub kind: Kind,
    pub submodel: Submodel,
    pub role: Role,
    pub datasets: BTreeMap<String, Verdict>,
    pub status: Status,
    pub quantities: BTreeMap<String, QuantityResult>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuantityResult {
    pub role: Role,
    pub status: Status,
    pub measured: Quantity,
    pub predicted: Option<Quantity>,
    pub residual: Option<f64>,
    pub source_tolerance: Option<Tolerance>,
    pub earlier_prediction: Option<Quantity>,
    pub allowed_earlier_residual: Option<f64>,
}

impl Corpus {
    pub fn run(&self, model: &(impl Model + ?Sized)) -> Report {
        let mut targets = Vec::new();
        for file in self.files() {
            let datasets: BTreeMap<_, _> = file
                .datasets
                .iter()
                .map(|name| (name.clone(), self.ledger().datasets[name].verdict))
                .collect();
            for target in &file.targets {
                let predicted = model.predict_with_operand_magnitudes(target, &file.conditions);
                let quantities: BTreeMap<_, _> = target
                    .measured
                    .iter()
                    .map(|(name, measured)| {
                        let quantity =
                            evaluate(&self.layers, target, name, measured, predicted.as_ref());
                        (name.clone(), quantity)
                    })
                    .collect();
                targets.push(TargetResult {
                    id: target.id.clone(),
                    source: file.source.clone(),
                    club: target
                        .inputs
                        .get("club")
                        .and_then(Quantity::as_str)
                        .map(str::to_owned),
                    shot_type: target
                        .inputs
                        .get("shot_type")
                        .and_then(Quantity::as_str)
                        .unwrap_or(target.kind.as_str())
                        .to_owned(),
                    kind: target.kind,
                    submodel: target.submodel,
                    role: target.role,
                    datasets: datasets.clone(),
                    status: quantities
                        .values()
                        .map(|quantity| quantity.status)
                        .max()
                        .unwrap_or(Status::ReportedOnly),
                    quantities,
                });
            }
        }
        targets.sort_by(|left, right| {
            (&left.source, &left.club, &left.shot_type, &left.id).cmp(&(
                &right.source,
                &right.club,
                &right.shot_type,
                &right.id,
            ))
        });
        Report {
            targets,
            skipped_datasets: self.skipped_datasets().to_vec(),
        }
    }
}

fn evaluate(
    layers: &Layers,
    target: &Target,
    name: &str,
    measured: &Quantity,
    predictions: Option<&(Quantities, BTreeMap<String, f64>)>,
) -> QuantityResult {
    let predicted = predictions
        .and_then(|(values, _)| values.get(name))
        .filter(|value| value.as_number().is_none_or(f64::is_finite));
    let operands = predictions
        .and_then(|(_, operands)| operands.get(name).copied())
        .filter(|magnitude| magnitude.is_finite() && *magnitude >= 0.0);
    let source_tolerance = target.tolerance.get(name).copied();
    let earlier_prediction = target
        .earlier
        .as_ref()
        .and_then(|earlier| earlier.predicted.get(name));
    let measured_number = measured.as_number();
    let predicted_number = predicted.and_then(Quantity::as_number);
    let residual = predicted_number
        .zip(measured_number)
        .map(|(prediction, measurement)| prediction - measurement)
        .filter(|value| value.is_finite());
    let allowed_earlier_residual = earlier_prediction
        .and_then(Quantity::as_number)
        .zip(measured_number)
        .zip(predicted_number)
        .map(|((earlier, measurement), prediction)| {
            (earlier - measurement).abs()
                + layers.allowance(target, name, earlier, prediction, operands)
        })
        .filter(|value| value.is_finite());
    let status = if predictions.is_none() {
        Status::NotRun
    } else if source_tolerance.is_none() && earlier_prediction.is_none() {
        Status::ReportedOnly
    } else {
        let pass = match measured {
            Quantity::Number(_) => residual.is_some_and(|residual| {
                source_tolerance
                    .zip(predicted_number)
                    .is_some_and(|(band, prediction)| band.contains(prediction, residual.abs()))
                    || allowed_earlier_residual.is_some_and(|bound| residual.abs() <= bound)
            }),
            Quantity::Bool(_) | Quantity::Text(_) => predicted == Some(measured),
        };
        if pass { Status::Passed } else { Status::Failed }
    };
    QuantityResult {
        role: target.role_for(name),
        status,
        measured: measured.clone(),
        predicted: predicted.cloned(),
        residual,
        source_tolerance,
        earlier_prediction: earlier_prediction.cloned(),
        allowed_earlier_residual,
    }
}

pub(crate) fn display_value(value: Option<impl fmt::Display>) -> String {
    value.map_or_else(|| "-".into(), |value| value.to_string())
}

pub(crate) fn display_skipped(
    f: &mut fmt::Formatter<'_>,
    skipped: &[SkippedDataset],
) -> fmt::Result {
    for entry in skipped {
        writeln!(f, "{entry}")?;
    }
    Ok(())
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "source\tclub\tshot type\ttarget\tquantity\trole\tpredicted\tmeasured\tresidual\tstatus"
        )?;
        for target in &self.targets {
            for (name, quantity) in &target.quantities {
                writeln!(
                    f,
                    "{}\t{}\t{}\t{}\t{name}\t{}\t{}\t{}\t{}\t{}",
                    target.source,
                    target.club.as_deref().unwrap_or("-"),
                    target.shot_type,
                    target.id,
                    quantity.role,
                    display_value(quantity.predicted.as_ref()),
                    quantity.measured,
                    display_value(quantity.residual),
                    quantity.status
                )?;
            }
        }
        display_skipped(f, &self.skipped_datasets)
    }
}
