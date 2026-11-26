use std::process::ExitCode;

use golf_sim_testkit::determinism::{LAYERS, bless, fixture_dir};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let target = match args {
        [command, target] if command == "bless" => target,
        _ => return Err("usage: golf-sim-testkit bless <layer>|--all".into()),
    };
    if target != "--all" && !LAYERS.iter().any(|(name, _)| name == target) {
        return Err(format!("unknown determinism layer: {target}"));
    }

    let dir = fixture_dir();
    for &(name, items) in LAYERS {
        if target == "--all" || target == name {
            let items = items();
            bless(&dir, name, &items)?;
            println!("blessed {name} ({} items)", items.len());
        }
    }
    Ok(())
}
