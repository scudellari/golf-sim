use std::{
    convert::Infallible,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use golf_sim_base::{
    corpus,
    digest::{object, sha256_hex},
    variant::{self, Array, Components, FloatWidth, Variant},
};
use golf_sim_testkit::tolerance::{self, Fixture};

const STEP: f64 = f32::EPSILON as f64;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golf_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus-cli");
        let output = Command::new(env!("CARGO"))
            .current_dir(workspace())
            .args(["build", "--offline", "-p", "golf-sim-cli", "--bin", "golf"])
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        target
            .join("debug")
            .join(format!("golf{}", std::env::consts::EXE_SUFFIX))
    })
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn golf_checks_corpus_tolerances_and_reports_drift_and_departures() {
    let golf = golf_binary();
    let work = TempDir::new();
    let fixture = workspace().join("testdata/variant/digests/fixture.bin");
    let invoke = |action: &str, path: &Path, layer: Option<&str>| {
        let mut command = Command::new(golf);
        command
            .current_dir(&work.0)
            .args(["corpus", action])
            .arg(path);
        if let Some(layer) = layer {
            command.args(["--layer", layer]);
        }
        command.output().unwrap()
    };
    let check = |path: &Path| invoke("check", path, Some("variant"));
    let passes = |output: Output, expected: &str| {
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        assert_eq!(text(&output.stdout), expected);
        print!("{expected}");
    };
    let fails = |output: Output, expected: &str| {
        assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
        assert_eq!(text(&output.stderr), expected);
        print!("{expected}");
    };

    let Variant::Dict(mut original) = corpus::read(&fixture).unwrap() else {
        panic!("expected corpus dictionary");
    };
    let Some(Variant::Array(cases)) = original.get_mut("cases") else {
        panic!("expected corpus cases");
    };
    let count = cases.items.len();
    passes(check(&fixture), &format!("checked {count} cases\n"));

    let Variant::Dict(first) = &mut cases.items[0] else {
        panic!("expected a snapshot");
    };
    *first.get_mut("digest").unwrap() = "edited".into();
    let edited = work.0.join("edited.bin");
    write_fixture(&edited, &Variant::Dict(original));
    fails(
        check(&edited),
        "golf: cases[0].output.digest (case \"null\"): discrete difference\n",
    );

    let config = work.0.join("testdata");
    fs::create_dir(&config).unwrap();
    let tolerances = config.join("tolerances.toml");
    let set_limits = |share: f64| {
        fs::write(
            &tolerances,
            format!(
                "[variant]\nabs = 0.0\nrel = 0.0\ndiscrete_share = {share:?}\n\
                 fields = {{ value = {{ abs = 0.125, rel = 0.0 }} }}\n"
            ),
        )
        .unwrap();
    };
    set_limits(0.0);
    let planted = work.0.join("planted.bin");
    let plant = |cases| write_fixture(&planted, &snapshots(cases));
    plant(vec![snapshot("numeric", 1.0625, 1.0, false)]);
    passes(check(&planted), "checked 1 case\n");

    plant(vec![snapshot("numeric", 1.25, 1.0, false)]);
    fails(
        check(&planted),
        "golf: cases[0].output.value (case \"numeric\"): abs 2.5e-1, rel 2e-1, steps 1.6777216e6\n",
    );

    plant(vec![
        snapshot("large-absolute", 8.0, 10.0, false),
        snapshot("large-relative", 1.0, 2.0, true),
    ]);
    let expected_drift = concat!(
        "value: max abs 2e0 (case \"large-absolute\"), max rel 5e-1 (case \"large-relative\"), ",
        "max steps 8.388608e6 (case \"large-relative\")\n",
        "cases[1].output.digest (case \"large-relative\"): discrete difference\n",
    );
    passes(invoke("drift", &planted, None), expected_drift);
    passes(invoke("drift", &planted, Some("variant")), expected_drift);

    plant(vec![
        snapshot("numeric", 1.0625, 1.0, false),
        snapshot("intentional", 2.0, 1.0, true),
    ]);
    fs::write(
        config.join("departures.toml"),
        "[[departure]]\nid = 'intentional'\nlayer = 'variant'\nchange = '2.0'\n",
    )
    .unwrap();
    passes(
        check(&planted),
        "skipped case \"intentional\" (variant; change 2.0)\nchecked 1 case\n",
    );
    passes(
        invoke("drift", &planted, None),
        concat!(
            "skipped case \"intentional\" (variant; change 2.0)\n",
            "value: max abs 6.25e-2 (case \"numeric\"), max rel 5.8823529411764705e-2 (case \"numeric\"), ",
            "max steps 4.934475294117647e5 (case \"numeric\")\n",
        ),
    );

    set_limits(0.5);
    plant(vec![
        snapshot("allowed", 1.0, 1.0, true),
        snapshot("match", 1.0, 1.0, false),
    ]);
    passes(
        check(&planted),
        concat!(
            "cases[0].output.digest (case \"allowed\"): discrete difference (within discrete allowance)\n",
            "checked 2 cases\n",
        ),
    );
    plant(vec![
        snapshot("allowed", 1.0, 1.0, true),
        snapshot("excess", 1.0, 1.0, true),
    ]);
    fails(
        check(&planted),
        "golf: cases[1].output.digest (case \"excess\"): discrete difference\n",
    );
    let nested = config.join("variant/fixture.bin");
    fs::create_dir(nested.parent().unwrap()).unwrap();
    fs::copy(&planted, &nested).unwrap();
    let cwd = work.0.join("other-working-directory");
    fs::create_dir(&cwd).unwrap();
    fails(
        Command::new(golf)
            .current_dir(cwd)
            .args(["corpus", "check"])
            .arg(nested)
            .args(["--layer", "variant"])
            .output()
            .unwrap(),
        "golf: cases[1].output.digest (case \"excess\"): discrete difference\n",
    );

    fails(
        invoke("check", &fixture, Some("unknown")),
        "golf: fixture key \"variant/digests\" belongs to layer \"variant\", requested \"unknown\"\n",
    );

    let limits = |abs, rel| {
        fs::write(
            &tolerances,
            format!(
                "[variant]\nabs = {abs:?}\nrel = {rel:?}\nf32_steps = 8\ndiscrete_share = 0.0\n"
            ),
        )
        .unwrap();
    };
    limits(0.0, 0.0);
    let at_steps = |id, steps| snapshot(id, 300.0, 300.0 + steps * STEP * 300.0, false);
    plant(vec![at_steps("seven-steps", 7.0)]);
    passes(check(&planted), "checked 1 case\n");
    plant(vec![at_steps("eight-steps", 8.0)]);
    passes(check(&planted), "checked 1 case\n");
    plant(vec![at_steps("nine-steps", 9.0)]);
    let output = check(&planted);
    assert_eq!(output.status.code(), Some(1));
    let failure = text(&output.stderr);
    assert!(
        failure.starts_with(
            "golf: cases[0].output.value (case \"nine-steps\"): abs 3.218650817871094e-4, rel "
        ),
        "{failure}"
    );
    assert!(failure.ends_with(", steps 9e0\n"), "{failure}");
    print!("{failure}");
    limits(0.0004, 0.0);
    passes(check(&planted), "checked 1 case\n");
    limits(0.0, 2e-6);
    passes(check(&planted), "checked 1 case\n");
    limits(0.0, 0.0);

    plant(vec![
        at_steps("seven-steps", 7.0),
        at_steps("nine-steps", 9.0),
    ]);
    let output = invoke("drift", &planted, None);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let drift = text(&output.stdout);
    assert!(
        drift.lines().any(
            |line| line.starts_with("value: max abs 3.218650817871094e-4")
                && line.ends_with("max steps 9e0 (case \"nine-steps\")")
        ),
        "{drift}"
    );
    print!("{drift}");
}

#[test]
fn golf_checks_and_reports_drift_for_every_phase_one_fixture() {
    let root = workspace().join("testdata");
    let golf = golf_binary();
    for layer in ["course", "physics", "world", "stroke"] {
        let mut fixtures: Vec<_> = fs::read_dir(root.join(layer))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("fixture.bin"))
            .filter(|path| path.is_file())
            .collect();
        assert!(!fixtures.is_empty(), "{layer}");
        fixtures.sort();

        for path in fixtures {
            let invoke = |action: &str, explicit_layer: bool| {
                let mut command = Command::new(golf);
                command.args(["corpus", action]).arg(&path);
                if explicit_layer {
                    command.args(["--layer", layer]);
                }
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{action} {}: {}",
                    path.display(),
                    text(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            };
            let check = invoke("check", false);
            let drift = invoke("drift", true);
            let checked = check.lines().last().unwrap();
            let checked_count: usize = checked
                .strip_prefix("checked ")
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(checked_count > 0, "{}", path.display());
            let skipped = |report: &str| {
                report
                    .lines()
                    .filter(|line| line.starts_with("skipped case "))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let envelope = corpus::read(&path).unwrap();
            let Some(Variant::Array(cases)) = envelope.as_dict().unwrap().get("cases") else {
                panic!("expected fixture cases");
            };
            let skips = skipped(&check);
            assert_eq!(checked_count + skips.len(), cases.items.len());
            assert_eq!(skips, skipped(&drift));
            let numeric_fields = drift
                .lines()
                .filter(|line| line.contains(": max abs "))
                .count();
            assert!(numeric_fields > 0, "{}", path.display());
            println!(
                "{}: {checked}, {} skipped, drift reported {numeric_fields} numeric fields",
                path.strip_prefix(&root).unwrap().display(),
                skips.len(),
            );
        }
    }

    let work = TempDir::new();
    let path = work.0.join("unknown.bin");
    write_fixture(&path, &fixture("physics/unregistered", Vec::new()));
    for action in ["check", "drift"] {
        let output = Command::new(golf)
            .args(["corpus", action])
            .arg(&path)
            .args(["--layer", "physics"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            text(&output.stderr),
            "golf: no corpus runner for fixture key \"physics/unregistered\"\n",
        );
        print!("{action}: {}", text(&output.stderr));
    }
}

#[test]
fn golf_resolves_course_id_and_revision_from_explicit_directories() {
    let root = workspace().join("testdata");
    let work = TempDir::new();
    let parent = work.0.join("courses");
    let bundle = parent.join("renamed");
    fs::create_dir_all(&bundle).unwrap();
    for name in ["manifest.json", "course.payload"] {
        fs::copy(
            root.join("courses/test-course").join(name),
            bundle.join(name),
        )
        .unwrap();
    }
    let Variant::Dict(mut header) = corpus::read(&root.join("course/grids/fixture.bin")).unwrap()
    else {
        panic!("expected fixture header");
    };
    let Some(Variant::Array(cases)) = header.get_mut("cases") else {
        panic!("expected fixture cases");
    };
    cases.items.truncate(1);
    let path = work.0.join("a-course.bin");
    write_fixture(&path, &Variant::Dict(header.clone()));

    let golf = golf_binary();
    for (action, dir) in [("check", &parent), ("drift", &bundle)] {
        let output = Command::new(golf)
            .current_dir(&work.0)
            .args(["corpus", action])
            .arg(&path)
            .arg("--course-dir")
            .arg(work.0.join("missing"))
            .arg("--course-dir")
            .arg(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(text(&output.stdout).contains(if action == "check" {
            "checked 1 case"
        } else {
            "max abs "
        }));
    }

    let Some(Variant::Dict(oracle)) = header.get_mut("oracle") else {
        panic!("expected oracle header");
    };
    let Some(Variant::Dict(course)) = oracle.get_mut("course") else {
        panic!("expected course identity");
    };
    *course.get_mut("revision").unwrap() = "missing-revision".into();
    write_fixture(&path, &Variant::Dict(header));
    for action in ["check", "drift"] {
        let output = Command::new(golf)
            .current_dir(&work.0)
            .args(["corpus", action])
            .arg(&path)
            .arg("--course-dir")
            .arg(&parent)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let error = text(&output.stderr);
        assert!(
            error.contains("course \"test-course\" revision \"missing-revision\" was not found"),
            "{error}"
        );
        print!("{action}: {error}");
    }
}

#[test]
fn cancelling_output_uses_its_field_rows_input_scale() {
    let work = TempDir::new();
    let config = work.0.join("testdata");
    fs::create_dir(&config).unwrap();
    let path = config.join("physics.bin");
    let output = |residual: f64| object([("result", object([("residual", residual.into())]))]);
    let input = object([
        (
            "points",
            Variant::Array(Array::new(vec![object([(
                "position",
                Variant::Vector3(double([1.0, -300.0, 0.0])),
            )])])),
        ),
        ("other", 100.0.into()),
    ]);
    write_fixture(
        &path,
        &fixture(
            "physics",
            vec![object([
                ("id", "cancellation".into()),
                ("input", input),
                ("output", output(0.0)),
            ])],
        ),
    );
    let row = |scale_from| {
        fs::write(
            config.join("tolerances.toml"),
            format!(
                "[physics]\nabs = 0.0\nrel = 0.0\nf32_steps = 1\ndiscrete_share = 0.0\n\
                 [physics.fields.result]\nf32_steps = 1\n\
                 [physics.fields.\"result.residual\"]\nf32_steps = 8\n{scale_from}"
            ),
        )
        .unwrap();
    };
    let run = |_: &variant::Dict| Ok::<_, Infallible>(output(7.0 * STEP * 300.0));
    let check = || tolerance::check(&path, "physics", run);
    let fixture = Fixture::read(&path).unwrap();
    row("");
    let error = check().unwrap_err().to_string();
    assert_eq!(
        error,
        "cases[0].output.result.residual (case \"cancellation\"): abs 2.503395080566406e-4, rel 1e0, steps 2.1e3"
    );
    println!("{error}");

    row("scale_from = ['input.points[0].position', 'input.other']\n");
    let report = check().unwrap();
    assert_eq!(report.checked, 1);
    print!("{report}");
    assert_eq!(
        fixture.drift("physics", run).unwrap().numeric["result.residual"]
            .steps
            .size,
        7.0
    );

    row("scale_from = ['input.points[1].position']\n");
    let error = tolerance::check(&path, "physics", |_| Ok::<_, Infallible>(output(0.0)))
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "cases[0] (case \"cancellation\"): scale_from path \"input.points[1].position\" is missing"
    );
    assert_eq!(
        fixture.drift("physics", run).unwrap_err().to_string(),
        error
    );
    println!("{error}");
}

fn double<T>(values: T) -> Components<T> {
    Components {
        values,
        width: FloatWidth::Double,
    }
}

#[test]
fn vector_components_use_their_own_expected_vectors() {
    let work = TempDir::new();
    let config = work.0.join("testdata");
    fs::create_dir(&config).unwrap();
    let path = config.join("course.bin");
    let output = |small, basis_component, sample_component| {
        object([
            ("flat", Variant::Vector2(double([300.0, small]))),
            ("point", Variant::Vector3(double([0.0, small, -300.0]))),
            ("wide", Variant::Vector4(double([300.0, 0.0, 1.0, small]))),
            (
                "transform",
                Variant::Transform3D(double([
                    300.0,
                    small,
                    0.0,
                    0.0,
                    1.0,
                    basis_component,
                    0.0,
                    0.0,
                    1.0,
                    small,
                    0.0,
                    -300.0,
                ])),
            ),
            (
                "samples",
                Variant::PackedVector3Array(double(vec![
                    [300.0, small, 0.0],
                    [1.0, sample_component, 0.0],
                ])),
            ),
        ])
    };
    write_fixture(
        &path,
        &fixture(
            "course",
            vec![object([
                ("id", "vectors".into()),
                ("output", output(0.0, 0.0, 0.0)),
            ])],
        ),
    );
    let close = 7.0 * STEP * 300.0;
    let run = |_: &variant::Dict| Ok::<_, Infallible>(output(close, 0.0, 0.0));
    assert_eq!(tolerance::check(&path, "course", run).unwrap().checked, 1);
    let report = Fixture::read(&path).unwrap().drift("course", run).unwrap();
    for path in [
        "flat.y",
        "point.y",
        "wide.w",
        "transform[1]",
        "transform[9]",
        "samples[0].y",
    ] {
        assert_eq!(report.numeric[path].steps.size, 7.0, "{path}");
    }

    let error = tolerance::check(&path, "course", |_| {
        Ok::<_, Infallible>(output(close, close, 0.0))
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with("cases[0].output.transform[5] "),
        "{error}"
    );
    assert!(error.ends_with(", steps 2.1e3"), "{error}");

    let error = tolerance::check(&path, "course", |_| {
        Ok::<_, Infallible>(output(close, 0.0, close))
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with("cases[0].output.samples[1].y "),
        "{error}"
    );
    assert!(error.ends_with(", steps 2.1e3"), "{error}");
}

#[test]
fn full_fixture_key_overrides_the_layer_row() {
    let work = TempDir::new();
    let testdata = work.0.join("testdata");
    fs::create_dir(&testdata).unwrap();
    fs::write(
        testdata.join("tolerances.toml"),
        r#"
[physics]
abs = 0.0
rel = 0.0
discrete_share = 0.0

["physics/strokes"]
abs = 0.125
rel = 0.0
f32_steps = 2
scale_from = ["input"]
discrete_share = 0.0
"#,
    )
    .unwrap();
    let path = testdata.join("fixture.bin");
    let write = |key| {
        write_fixture(
            &path,
            &fixture(
                key,
                vec![object([
                    ("id", "same-input".into()),
                    ("input", 8.0.into()),
                    ("output", 1.0.into()),
                ])],
            ),
        );
    };
    let run = |_: &variant::Dict| Ok::<_, Infallible>(1.125.into());
    write("physics/strokes");
    assert_eq!(tolerance::check(&path, "physics", run).unwrap().checked, 1);
    let drift = Fixture::read(&path).unwrap().drift("physics", run).unwrap();
    assert_eq!(drift.numeric["$"].steps.size, 131_072.0);

    write("physics/contact");
    let error = tolerance::check(&path, "physics", run).unwrap_err();
    assert!(error.to_string().contains("abs 1.25e-1"), "{error}");
    let drift = Fixture::read(&path).unwrap().drift("physics", run).unwrap();
    assert_eq!(drift.numeric["$"].steps.size, 1_048_576.0);
}

#[test]
fn indexed_field_bounds_match_any_index_and_prefer_literals() {
    let work = TempDir::new();
    let testdata = work.0.join("testdata");
    fs::create_dir(&testdata).unwrap();
    fs::write(
        testdata.join("tolerances.toml"),
        r#"
[physics]
abs = 0.0
rel = 0.0
discrete_share = 0.0

[physics.fields]
events = { abs = 0.5 }
"events[*].time_s" = { abs = 0.25 }
"events[0].time_s" = { abs = 0.125 }
"#,
    )
    .unwrap();
    let path = testdata.join("fixture.bin");
    let output = |first, last| {
        let events = (0..=10)
            .map(|index| {
                let time = match index {
                    0 => first,
                    10 => last,
                    _ => 1.0,
                };
                object([("time_s", Variant::from(time))])
            })
            .collect();
        object([("events", Variant::Array(Array::new(events)))])
    };
    write_fixture(
        &path,
        &fixture(
            "physics",
            vec![object([
                ("id", "events".into()),
                ("output", output(1.0, 1.0)),
            ])],
        ),
    );
    let check = |first, last| {
        tolerance::check(&path, "physics", |_| {
            Ok::<_, Infallible>(output(first, last))
        })
    };
    assert_eq!(check(1.125, 1.25).unwrap().checked, 1);
    for (first, last, field) in [
        (1.25, 1.25, "events[0].time_s"),
        (1.125, 1.5, "events[10].time_s"),
    ] {
        let error = check(first, last).unwrap_err();
        assert!(
            error.to_string().contains(&format!(".output.{field} ")),
            "{error}"
        );
    }
}

#[test]
fn shared_check_uses_the_scalar_ulp_bound_from_the_math_layer() {
    let work = TempDir::new();
    let path = work.0.join("math.bin");
    let fixture = fixture(
        "math",
        vec![object([("id", "scalar".into()), ("output", 1.0.into())])],
    );
    write_fixture(&path, &fixture);
    let check = |steps| {
        tolerance::check(&path, "math", |_| {
            Ok::<_, Infallible>(f64::from_bits(1.0_f64.to_bits() + steps).into())
        })
    };
    assert_eq!(check(4).unwrap().checked, 1);
    assert!(
        check(5)
            .unwrap_err()
            .to_string()
            .starts_with("cases[0].output (case \"scalar\"): abs ")
    );
}

fn snapshot(id: &str, expected: f64, input: f64, edit_digest: bool) -> Variant {
    let bytes = variant::encode(&Variant::from(input)).unwrap();
    let digest = if edit_digest {
        "edited".into()
    } else {
        sha256_hex(&bytes)
    };
    object([
        ("id", id.into()),
        ("op", "snapshot".into()),
        ("value", expected.into()),
        ("bytes", Variant::PackedByteArray(bytes)),
        ("digest", Variant::String(digest)),
    ])
}

fn snapshots(cases: Vec<Variant>) -> Variant {
    fixture("variant/digests", cases)
}

fn fixture(layer: &str, cases: Vec<Variant>) -> Variant {
    object([
        ("format", 1_i64.into()),
        ("layer", layer.into()),
        (
            "oracle",
            object([
                ("versions", object([])),
                ("commit", "example".into()),
                ("hash", "example".into()),
            ]),
        ),
        ("cases", Variant::Array(Array::new(cases))),
    ])
}

fn write_fixture(path: &Path, fixture: &Variant) {
    let payload = variant::encode(fixture).unwrap();
    let mut bytes = b"GSCORP01".to_vec();
    bytes.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(payload);
    fs::write(path, bytes).unwrap();
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "golf-corpus-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("{}: {error}", path.display()),
            }
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
