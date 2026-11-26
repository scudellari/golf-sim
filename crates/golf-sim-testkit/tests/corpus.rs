use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim_base::{
    corpus,
    digest::{object, sha256_hex},
    variant::{self, Array, Variant},
};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golf_binary() -> PathBuf {
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
        let mut command = Command::new(&golf);
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
        "golf: cases[0].output.value (case \"numeric\"): abs 2.5e-1, rel 2e-1\n",
    );

    plant(vec![
        snapshot("large-absolute", 8.0, 10.0, false),
        snapshot("large-relative", 1.0, 2.0, true),
    ]);
    let expected_drift = concat!(
        "value: max abs 2e0 (case \"large-absolute\"), max rel 5e-1 (case \"large-relative\")\n",
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
            "value: max abs 6.25e-2 (case \"numeric\"), max rel 5.8823529411764705e-2 (case \"numeric\")\n",
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
        Command::new(&golf)
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
        "golf: unknown corpus layer \"unknown\"\n",
    );
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
        golf_sim_testkit::tolerance::check(&path, "math", |_| {
            Ok::<_, std::convert::Infallible>(f64::from_bits(1.0_f64.to_bits() + steps).into())
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
