use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use proc_macro2::{TokenStream, TokenTree};

const TABLES: [&str; 2] = ["tolerances.toml", "departures.toml"];

fn testkit() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn workspace() -> PathBuf {
    testkit().join("../..").canonicalize().unwrap()
}

fn run(command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} exited with {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn packaged_tables_match_the_workspace_tables() {
    let workspace = workspace();
    for table in TABLES {
        let source = Path::new("testdata").join(table);
        let packaged = Path::new("crates/golf-sim-testkit").join(table);
        let read = |path: &Path| {
            fs::read(workspace.join(path))
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        };
        assert!(
            read(&source) == read(&packaged),
            "Copy {} to {} after editing the workspace table.",
            source.display(),
            packaged.display()
        );
    }
}

#[test]
fn cargo_package_lists_both_tables() {
    let files = run(Command::new(env!("CARGO")).current_dir(workspace()).args([
        "package",
        "--list",
        "--offline",
        "--allow-dirty",
        "-p",
        "golf-sim-testkit",
    ]));
    for table in TABLES {
        assert!(
            files.lines().any(|path| path == table),
            "{table} missing:\n{files}"
        );
        println!("cargo package lists {table}");
    }
}

fn literal_path(repr: &str) -> Option<&str> {
    if let Some(raw) = repr.strip_prefix('r') {
        let (hashes, value) = raw.split_once('"')?;
        return value.strip_suffix(hashes)?.strip_suffix('"');
    }
    let value = repr.strip_prefix('"')?.strip_suffix('"')?;
    (!value.contains('\\')).then_some(value)
}

fn check_includes(krate: &Path, file: &Path, tokens: TokenStream) -> Result<(), String> {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        let TokenTree::Ident(ident) = token else {
            if let TokenTree::Group(group) = token {
                check_includes(krate, file, group.stream())?;
            }
            continue;
        };
        let name = ident.to_string();
        if !matches!(
            name.trim_start_matches("r#"),
            "include_str" | "include_bytes"
        ) || !matches!(tokens.peek(), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
        {
            continue;
        }
        let location = format!("{}:{}", file.display(), ident.span().start().line);
        let literal_required = || {
            format!(
                "{location}: {ident}! requires a plain or raw path literal for the crate boundary check"
            )
        };
        tokens.next();
        let Some(TokenTree::Group(arguments)) = tokens.next() else {
            return Err(literal_required());
        };
        let mut arguments = arguments.stream().into_iter();
        let Some(TokenTree::Literal(literal)) = arguments.next() else {
            return Err(literal_required());
        };
        match arguments.next() {
            None => {}
            Some(TokenTree::Punct(punct))
                if punct.as_char() == ',' && arguments.next().is_none() => {}
            _ => return Err(literal_required()),
        }
        let repr = literal.to_string();
        let path = literal_path(&repr).ok_or_else(literal_required)?;
        let target = file
            .parent()
            .unwrap()
            .join(path)
            .canonicalize()
            .map_err(|error| {
                format!("{location}: cannot resolve {ident}! path {path:?}: {error}")
            })?;
        if !target.starts_with(krate) {
            return Err(format!(
                "{location}: {ident}! path {path:?} resolves outside the crate"
            ));
        }
    }
    Ok(())
}

fn check_sources(krate: &Path, path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            check_sources(krate, &entry.unwrap().path());
        }
    } else if path.extension().is_some_and(|ext| ext == "rs") && path.is_file() {
        let source = fs::read_to_string(path).unwrap();
        let tokens = source
            .parse()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        check_includes(krate, path, tokens).unwrap_or_else(|error| panic!("{error}"));
    }
}

fn check_crates(dir: &Path) {
    if dir.join("Cargo.toml").is_file() {
        let krate = dir.canonicalize().unwrap();
        // A consumer compiles only these. Tests and benches may read the workspace's testdata.
        for entry in ["src", "build.rs"] {
            check_sources(&krate, &krate.join(entry));
        }
    }
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() && entry.file_name() != "target" {
            check_crates(&entry.path());
        }
    }
}

#[test]
fn embedded_files_stay_inside_their_crate() {
    check_crates(&workspace().join("crates"));
    println!("Every include_str! and include_bytes! a consumer compiles stays inside its crate");
}

#[test]
fn crate_boundary_check_reports_each_macro_and_its_line() {
    let krate = testkit().canonicalize().unwrap();
    let file = krate.join("src/tolerance.rs");
    for name in ["include_str", "include_bytes"] {
        let source =
            format!("mod example {{\n{name}!\n(r#\"../../../testdata/tolerances.toml\"#);\n}}");
        let error = check_includes(&krate, &file, source.parse().unwrap()).unwrap_err();
        assert!(
            error.contains(&format!("{}:2: {name}! path", file.display())),
            "{error}"
        );
        assert!(error.ends_with("resolves outside the crate"), "{error}");
    }
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(parent: &Path) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = parent.join(format!("package-{}-{timestamp}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_workspace(source: &Path, destination: &Path) {
    let tracked = run(Command::new("git")
        .current_dir(source)
        .args(["ls-files", "--cached", "-z"]));
    let workspace_tables = TABLES.map(|table| Path::new("testdata").join(table));
    let packaged_tables = TABLES.map(|table| Path::new("crates/golf-sim-testkit").join(table));
    // Use working tree bytes so the test can check a patch before it is committed.
    for path in tracked
        .split_terminator('\0')
        .map(Path::new)
        .filter(|path| {
            !workspace_tables
                .iter()
                .chain(&packaged_tables)
                .any(|table| table == path)
        })
        .chain(packaged_tables.iter().map(PathBuf::as_path))
    {
        let copy = destination.join(path);
        fs::create_dir_all(copy.parent().unwrap()).unwrap();
        fs::copy(source.join(path), &copy)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn vendored_consumer_builds_golf_offline() {
    let target_tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let scratch = ScratchDir::new(target_tmp);
    // Sibling workspaces let Cargo resolve the copied crates' inherited fields.
    let consumer = scratch.0.join("consumer");
    let vendored = scratch.0.join("vendor/golf-sim");
    copy_workspace(&workspace(), &vendored);
    fs::create_dir_all(consumer.join("src")).unwrap();
    fs::copy(vendored.join("Cargo.lock"), consumer.join("Cargo.lock")).unwrap();
    fs::write(
        consumer.join("Cargo.toml"),
        r#"[package]
name = "golf-sim-consumer"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
golf-sim-testkit = { path = "../vendor/golf-sim/crates/golf-sim-testkit" }
"#,
    )
    .unwrap();
    fs::write(
        consumer.join("src/main.rs"),
        "use golf_sim_testkit::tolerance;\nfn main() { let _ = tolerance::variant_snapshot; }\n",
    )
    .unwrap();

    // Running from under target/ inherits the checkout's Cargo source configuration.
    let target = target_tmp.join("package-build");
    let build = || {
        let mut cargo = Command::new(env!("CARGO"));
        cargo
            .current_dir(&consumer)
            .args(["build", "--offline", "--target-dir"])
            .arg(&target);
        cargo
    };
    run(&mut build());
    run(build()
        .arg("--manifest-path")
        .arg(vendored.join("Cargo.toml"))
        .args(["-p", "golf-sim-cli", "--bin", "golf", "--locked"]));

    let golf = target
        .join("debug")
        .join(format!("golf{}", std::env::consts::EXE_SUFFIX));
    let fixture = consumer.join("fixture.bin");
    fs::copy(
        vendored.join("testdata/variant/digests/fixture.bin"),
        &fixture,
    )
    .unwrap();
    let report = run(Command::new(golf)
        .current_dir(&consumer)
        .args(["corpus", "check"])
        .arg(fixture)
        .args(["--layer", "variant"]));
    let summary = report.lines().last().unwrap_or_default();
    assert!(summary.starts_with("checked "), "{report}");
    println!("Vendored consumer built golf offline: {summary}");
}
