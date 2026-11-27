use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim_validate::{
    Conditions, Corpus, Kind, Model, Quantities, Quantity, Report, Role, Scorecard, ScorecardDiff,
    Status, Submodel, Target, TargetFile, Verdict,
};
use serde_json::{Value, json};

fn measured(target: &Target, _: &Conditions) -> Option<Quantities> {
    Some(target.measured.clone())
}

fn earlier(target: &Target, _: &Conditions) -> Option<Quantities> {
    Some(target.earlier.as_ref()?.predicted.clone())
}

#[test]
fn committed_corpus_loads_reports_and_scores() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/validation");
    let corpus = Corpus::load(&root).unwrap();
    let files: BTreeSet<_> = corpus
        .ledger()
        .datasets
        .values()
        .filter(|dataset| dataset.verdict != Verdict::Excluded)
        .flat_map(|dataset| {
            dataset
                .files
                .iter()
                .map(|path| root.join("targets").join(path))
                .chain(
                    dataset
                        .local_files
                        .iter()
                        .map(|path| root.join("local").join(path)),
                )
        })
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(corpus.files().len(), files.len());
    let targets: Vec<_> = files
        .into_iter()
        .flat_map(|path| {
            serde_json::from_slice::<TargetFile>(&fs::read(path).unwrap())
                .unwrap()
                .targets
        })
        .collect();
    assert!(!targets.is_empty());
    let checked = targets
        .iter()
        .filter(|target| !target.tolerance.is_empty() || target.earlier.is_some())
        .count();
    let with_earlier = targets
        .iter()
        .filter(|target| target.earlier.is_some())
        .count();
    let report = corpus.run(&measured);
    for dataset in &report.skipped_datasets {
        println!("{dataset}");
    }
    assert_eq!(report.targets.len(), targets.len());
    assert_eq!(count(&report, Status::Passed), checked);
    assert_eq!(
        count(&report, Status::ReportedOnly),
        targets.len() - checked
    );
    assert_eq!(count(&report, Status::Failed), 0);
    assert_eq!(count(&report, Status::NotRun), 0);
    let sample_club = if report
        .skipped_datasets
        .iter()
        .any(|dataset| dataset.dataset == "local-deliveries")
    {
        "lw"
    } else {
        "driver"
    };
    let sample_prefix =
        format!("PING\t{sample_club}\tdelivery\tPING.{sample_club}.face-share\tface_share\t");
    let text = report.to_string();
    let sample = text
        .lines()
        .find(|line| line.starts_with(&sample_prefix))
        .unwrap();
    assert!(sample.ends_with("\t0\tpass"), "{sample}");
    let json = serde_json::to_string(&report).unwrap();
    let decoded: Report = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.targets.len(), report.targets.len());

    let scorecard = report.scorecard();
    let covered: BTreeSet<_> = scorecard
        .rows
        .iter()
        .flat_map(|row| row.scores.datasets.keys().map(String::as_str))
        .collect();
    let with_files: BTreeSet<_> = corpus
        .files()
        .iter()
        .flat_map(|file| file.datasets.iter().map(String::as_str))
        .collect();
    assert_eq!(covered, with_files);
    for row in &scorecard.rows {
        assert!(row.scores.score.is_none_or(|score| score == 0.0));
        for quantity in row.scores.quantities.values() {
            for stats in [quantity.residual, quantity.share_of_tolerance]
                .into_iter()
                .flatten()
            {
                assert_eq!((stats.mean_absolute, stats.rms), (0.0, 0.0));
            }
        }
    }
    let normalized: Vec<_> = scorecard
        .submodels
        .iter()
        .filter_map(|row| row.score)
        .collect();
    assert!(normalized.iter().all(|&score| score == 0.0));
    let json = serde_json::to_string(&scorecard).unwrap();
    assert_eq!(serde_json::from_str::<Scorecard>(&json).unwrap(), scorecard);
    assert!(scorecard.to_string().contains("BIB\tturf_bounce\tfit"));

    let baseline = corpus.run(&earlier);
    assert_eq!(
        count(&baseline, Status::NotRun),
        targets.len() - with_earlier
    );
    assert_eq!(count(&baseline, Status::Failed), 0);
    println!(
        "{} targets · {} pass · {} reported only · {} ledger datasets with rows · normalized scores {normalized:?}",
        report.targets.len(),
        count(&report, Status::Passed),
        count(&report, Status::ReportedOnly),
        covered.len(),
    );
    println!("{sample}");
    println!("Earlier predictions · {with_earlier} run");
}

#[test]
fn differences_allow_single_precision_rounding_at_both_operands() {
    let fixture = Fixture::new("[datasets.planted]\nverdict = 'cite'\nfiles = ['a.json']");
    fixture.write(
        "tolerances.toml",
        "[physics]\nrel = 0.00001\nf32_steps = 8\n[stroke]\nabs = 0.0",
    );
    let mut small = target("difference");
    small["measured"] = json!({"spin_rpm_change": 0.0});
    small["tolerance"] = json!({});
    small["earlier"]["predicted"] = json!({"spin_rpm_change": 0.0});
    fixture.targets("targets/a.json", "A", "planted", "cite", vec![small]);

    struct Difference;
    impl Model for Difference {
        fn predict(&self, _: &Target, _: &Conditions) -> Option<Quantities> {
            Some(BTreeMap::from([(
                "spin_rpm_change".into(),
                Quantity::Number(0.003),
            )]))
        }

        fn predict_with_operand_magnitudes(
            &self,
            target: &Target,
            conditions: &Conditions,
        ) -> Option<(Quantities, BTreeMap<String, f64>)> {
            Some((
                self.predict(target, conditions)?,
                BTreeMap::from([("spin_rpm_change".into(), 5000.003)]),
            ))
        }
    }

    let corpus = fixture.load().unwrap();
    let without = corpus
        .run(&(|target: &Target, conditions: &Conditions| Difference.predict(target, conditions)));
    assert_eq!(without.targets[0].status, Status::Failed);
    let with = corpus.run(&Difference);
    let quantity = &with.targets[0].quantities["spin_rpm_change"];
    assert_eq!(quantity.status, Status::Passed);
    assert!(
        (quantity.allowed_earlier_residual.unwrap() - 5000.003 * 8.0 / 8388608.0).abs() < 1e-12
    );
}

#[test]
fn unlisted_nested_data_fails_the_ledger_check() {
    let fixture = Fixture::new("");
    fixture.write("validation/targets/unlisted/data.csv", "data");
    let error = fixture.load().unwrap_err();
    assert!(error.path.ends_with(Path::new("unlisted").join("data.csv")));
    assert_eq!(error.message, "file has no licence ledger entry");
    println!("{error}");
}

#[test]
fn a_dataset_with_data_but_no_targets_fails_the_ledger_check() {
    for (verdict, sources, directory, files) in [
        ("commit", "[]", "targets", "files"),
        ("cite", "[]", "targets", "files"),
        ("fetch", "[]", "targets", "files"),
        ("local", "['P']", "local", "local_files"),
    ] {
        let fixture = Fixture::new(&format!(
            "[datasets.planted]\nverdict = '{verdict}'\nsources = {sources}\n{files} = ['raw.csv']"
        ));
        fixture.write(&format!("validation/{directory}/raw.csv"), "data");
        let error = fixture.load().unwrap_err().to_string();
        assert!(
            error.contains("datasets with files give no scorecard row: {\"planted\"}"),
            "{verdict}: {error}"
        );
    }
}

#[test]
fn local_fit_inputs_without_sources_need_no_scorecard_row() {
    let fixture = Fixture::new(
        "
[datasets.planted]
verdict = 'cite'
sources = ['A']
files = ['a.json']
[datasets.fit-inputs]
verdict = 'local'
sources = []
local_files = ['raw/data.csv', 'raw/SOURCE.txt']
",
    );
    fixture.targets(
        "targets/a.json",
        "A",
        "planted",
        "cite",
        vec![target("A.first")],
    );
    let without = fixture.load().unwrap().run(&measured).scorecard();
    assert_eq!(without.skipped_datasets.len(), 1);
    assert_eq!(without.skipped_datasets[0].dataset, "fit-inputs");

    fixture.write("validation/local/raw/data.csv", "data");
    fixture.write("validation/local/raw/SOURCE.txt", "provenance");
    let with = fixture.load().unwrap().run(&measured).scorecard();
    assert!(with.skipped_datasets.is_empty());
    assert_eq!(with.rows, without.rows);
    assert_eq!(with.submodels, without.submodels);
}

#[test]
fn absent_fetch_and_local_are_listed_and_excluded_data_is_unread() {
    let fixture = Fixture::new(
        "
[datasets.remote]
verdict = 'fetch'
fetch = { url = 'https://example.test/data.csv', step = 'Download to a local cache' }
[datasets.local]
verdict = 'local'
local_files = ['present.json', 'absent.json']
[datasets.unused]
verdict = 'excluded'
files = ['excluded.json']
",
    );
    fixture.write(
        "validation/targets/excluded.json",
        "excluded data is unread",
    );
    fixture.targets(
        "local/present.json",
        "P",
        "local",
        "local",
        vec![target("P.present")],
    );
    let corpus = fixture.load().unwrap();
    let report = corpus.run(&measured);
    assert_eq!(count(&report, Status::Passed), 1);
    assert_eq!(report.skipped_datasets.len(), 2);
    let local = &report.skipped_datasets[0];
    assert_eq!(local.dataset, "local");
    assert_eq!(local.verdict, Verdict::Local);
    assert_eq!(local.missing_files, ["local/absent.json"]);
    let fetch = &report.skipped_datasets[1];
    assert_eq!(fetch.dataset, "remote");
    assert_eq!(fetch.verdict, Verdict::Fetch);
    let row = &report.scorecard().rows[0];
    assert_eq!(row.scores.datasets["local"], Verdict::Local);
    for line in report
        .to_string()
        .lines()
        .filter(|line| line.starts_with("skipped"))
    {
        println!("{line}");
    }
    assert!(
        report
            .to_string()
            .contains("skipped\tremote\tfetch\tno local data")
    );
}

#[test]
fn unknown_typed_values_name_the_file_and_target() {
    let fixture = Fixture::new("[datasets.planted]\nverdict = 'cite'\nfiles = ['a.json']");
    for (field, unknown) in [
        ("kind", json!("mystery")),
        ("submodel", json!("mystery")),
        ("use", json!("mystery")),
        ("tolerance", json!({"carry_yd":{"mystery":1.0}})),
        (
            "earlier",
            json!({"via":"mystery", "predicted":{}, "residual":{}}),
        ),
    ] {
        let mut value = target("A.invalid");
        value[field] = unknown;
        fixture.targets("targets/a.json", "A", "planted", "cite", vec![value]);
        let error = fixture.load().unwrap_err().to_string();
        assert!(error.contains("a.json: target \"A.invalid\""), "{error}");
        assert!(error.contains("unknown variant `mystery`"), "{error}");
    }
}

#[test]
fn per_quantity_bounds_and_reported_only_status() {
    let fixture = Fixture::new("[datasets.planted]\nverdict = 'cite'\nfiles = ['a.json']");
    fixture.write(
        "tolerances.toml",
        "[physics]\nabs = 0.0\n[stroke]\nabs = 0.0\n[stroke.fields.carry_m]\nabs = 0.1",
    );
    let mut targets = Vec::new();
    for (id, band, earlier, submodel) in [
        ("a.sd", json!({"sd": 1.0}), 10.0, "flight"),
        ("a.ci95", json!({"ci95": 2.0}), 10.0, "flight"),
        ("a.range", json!({"range": [8.0, 12.0]}), 10.0, "flight"),
        ("b.earlier", json!({"ci95": 1.0}), 12.0, "stroke"),
        ("c.fail", json!({"ci95": 1.0}), 12.0, "flight"),
    ] {
        let mut value = target(id);
        value["submodel"] = json!(submodel);
        value["tolerance"]["carry_yd"] = band;
        value["earlier"]["predicted"]["carry_yd"] = json!(earlier);
        targets.push(value);
    }
    let mut unbounded = target("d.reported");
    unbounded.as_object_mut().unwrap().remove("tolerance");
    unbounded["earlier"] = Value::Null;
    unbounded["check"] = json!({"carry_yd": {"abs": 0.0}});
    targets.push(unbounded);
    let mut discrete = target("e.discrete");
    discrete["kind"] = json!("cup_capture");
    discrete["submodel"] = json!("cup");
    discrete["measured"] = json!({"holed": true});
    discrete["tolerance"] = json!({});
    discrete["earlier"] = json!({"via":"production", "predicted":{"holed": true}, "residual":{}});
    targets.push(discrete);
    fixture.targets("targets/a.json", "A", "planted", "cite", targets);
    let corpus = fixture.load().unwrap();

    let model = |target: &Target, _: &Conditions| {
        let carry = if target.id.starts_with("a.") {
            12.0
        } else {
            12.0625
        };
        if target.kind == Kind::CupCapture {
            return Some([("holed".into(), Quantity::Bool(false))].into());
        }
        Some([("carry_yd".into(), Quantity::Number(carry))].into())
    };
    let report = corpus.run(&model);
    assert_eq!(count(&report, Status::Passed), 4);
    assert_eq!(count(&report, Status::ReportedOnly), 1);
    assert_eq!(count(&report, Status::Failed), 2);
    let failed = report
        .targets
        .iter()
        .find(|target| target.id == "c.fail")
        .unwrap();
    let carry = &failed.quantities["carry_yd"];
    assert_eq!(carry.status, Status::Failed);
    assert_eq!(carry.allowed_earlier_residual, Some(2.0));
    assert!(carry.residual.unwrap() > 2.0);
    let cup = report
        .targets
        .iter()
        .find(|target| target.id == "e.discrete")
        .unwrap();
    assert_eq!(cup.status, Status::Failed);
    assert_eq!(cup.quantities["holed"].residual, None);
    println!(
        "{}",
        report
            .to_string()
            .lines()
            .find(|line| line.contains("c.fail"))
            .unwrap()
    );

    let uncovered = corpus.run(&(|_: &Target, _: &Conditions| None));
    assert_eq!(count(&uncovered, Status::NotRun), 7);
    assert_eq!(count(&uncovered, Status::Failed), 0);
}

#[test]
fn split_quantities_score_in_their_role_and_diff_names_changed_rows() {
    let fixture = Fixture::new(
        "
[datasets.planted]
verdict = 'cite'
files = ['a.json','b.json']
",
    );
    let mut split = target("A.first");
    split["measured"]["apex_yd"] = json!(5.0);
    split["tolerance"]["apex_yd"] = json!({"sd": 2.0});
    split["held_out"] = json!(["apex_yd"]);
    fixture.targets(
        "targets/a.json",
        "A",
        "planted",
        "cite",
        vec![split, target("A.second")],
    );
    fixture.targets(
        "targets/b.json",
        "B",
        "planted",
        "cite",
        vec![target("B.first")],
    );
    let corpus = fixture.load().unwrap();
    let before = corpus.run(&measured).scorecard();
    let changed = |target: &Target, _: &Conditions| {
        let mut values = target.measured.clone();
        if target.id == "A.first" {
            values.insert("carry_yd".into(), Quantity::Number(12.0));
            values.insert("apex_yd".into(), Quantity::Number(6.0));
        }
        Some(values)
    };
    let after = corpus.run(&changed).scorecard();
    let fit = &after.rows[0].scores;
    assert_eq!(fit.counts.targets, 2);
    assert_eq!(fit.counts.passes, 2);
    assert_eq!(fit.quantities.keys().collect::<Vec<_>>(), ["carry_yd"]);
    let residual = fit.quantities["carry_yd"].residual.unwrap();
    assert_eq!(residual.count, 2);
    assert_eq!(residual.mean_absolute, 1.0);
    assert_eq!(residual.rms, std::f64::consts::SQRT_2);
    let normalized = fit.quantities["carry_yd"].share_of_tolerance.unwrap();
    assert_eq!(normalized.mean_absolute, 0.5);
    assert_eq!(normalized.rms, std::f64::consts::SQRT_2 / 2.0);
    let held_out = &after.rows[1].scores;
    assert_eq!(held_out.counts.targets, 1);
    assert_eq!(held_out.quantities.keys().collect::<Vec<_>>(), ["apex_yd"]);
    assert_eq!(held_out.score, Some(0.25));
    assert_eq!(after.submodels[0].score, Some(0.25));
    assert_eq!(after.submodels[1].score, Some(0.25));

    let diff = Scorecard::diff(&before, &after);
    let keys: Vec<_> = diff
        .rows
        .iter()
        .map(|row| (row.key.source.as_str(), row.key.submodel, row.key.role))
        .collect();
    assert_eq!(
        keys,
        [
            ("A", Submodel::Flight, Role::Fit),
            ("A", Submodel::Flight, Role::HeldOut)
        ]
    );
    assert_eq!(diff.submodels.len(), 2);
    assert!(diff.to_string().contains("after\tA\tflight\theld-out"));
    assert!(!diff.to_string().contains("\tB\t"));
    let json = serde_json::to_string(&diff).unwrap();
    assert_eq!(serde_json::from_str::<ScorecardDiff>(&json).unwrap(), diff);
    assert!(Scorecard::diff(&after, &after).rows.is_empty());
    println!("Changed rows · A flight fit · A flight held-out");
}

fn count(report: &Report, status: Status) -> usize {
    report
        .targets
        .iter()
        .filter(|target| target.status == status)
        .count()
}

fn target(id: &str) -> Value {
    json!({
        "id": id, "kind": "launch", "submodel": "flight", "use": "fit",
        "inputs": {"club": "driver"}, "measured": {"carry_yd": 10.0},
        "tolerance": {"carry_yd": {"ci95": 2.0}},
        "earlier": {"via":"production", "predicted":{"carry_yd":10.0}, "residual":{"carry_yd":0.0}}
    })
}

struct Fixture(PathBuf);

impl Fixture {
    fn new(datasets: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("golf-validation-{}-{id}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => break root,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("{error}"),
            }
        };
        let fixture = Self(root);
        fixture.write(
            "validation/targets/LICENCES.toml",
            &format!("format = 1\n{datasets}"),
        );
        fixture.write(
            "tolerances.toml",
            "[physics]\nabs = 0.0\n[stroke]\nabs = 0.0",
        );
        fixture
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn targets(&self, path: &str, source: &str, dataset: &str, verdict: &str, targets: Vec<Value>) {
        let file = json!({
            "format": 1, "source": source, "citation": "Planted data", "url": "https://example.test/",
            "datasets": [dataset], "verdict": verdict, "targets": targets,
            "conditions": {
                "air_temp_c": 25.0, "pressure_pa": 101325.0, "humidity": 0.5, "wind_mph": 0.0,
                "ball": "standard", "lie": "tee",
                "ground": {"surface":"fairway", "firmness":0.5, "moisture":0.3},
                "green": {"stimp_ft":10.0, "firmness":0.5, "moisture":0.3}
            }
        });
        self.write(&format!("validation/{path}"), &file.to_string());
    }

    fn load(&self) -> Result<Corpus, golf_sim_validate::LoadError> {
        Corpus::load(self.0.join("validation"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
