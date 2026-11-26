use std::process::Command;

const SUBCOMMANDS: &[&str] = &[
    "play",
    "replay",
    "schema",
    "batch",
    "stroke",
    "course",
    "day",
    "days",
    "digests",
    "golden",
    "probe",
    "reach",
    "bag",
    "lie-table",
    "fat-table",
    "sweep-swing",
    "tee-table",
    "strategy",
    "play-course",
    "fit",
    "corpus",
];

#[test]
fn the_golf_program_has_every_planned_subcommand() {
    for name in SUBCOMMANDS {
        let output = Command::new(env!("CARGO_BIN_EXE_golf"))
            .args([name, "--help"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "golf {name} --help: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
