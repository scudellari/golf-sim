use std::hint::black_box;
use std::time::{Duration, Instant};

use golf_sim_base::trace::{NoProbe, Phase, Probe, Source};

const STEPS: usize = 1_000_000;
const RUNS: usize = 5;

fn record_step<P: Probe>(probe: &mut P, speed: f64) {
    probe.value("speed", speed, "m/s", Source::Computed);
    if P::ENABLED {
        let energy = black_box(0.5 * speed * speed);
        probe.value("specific_kinetic_energy", energy, "J/kg", Source::Computed);
    }
}

#[inline(never)]
fn run<const WITH_PROBE: bool>(mut speed: f64) -> f64 {
    let mut probe = NoProbe;
    if WITH_PROBE {
        probe.begin_phase(Phase::Roll);
    }
    for _ in 0..STEPS {
        speed = speed * 0.999_999 + 0.000_001;
        if WITH_PROBE {
            record_step(&mut probe, speed);
        }
    }
    speed
}

fn measure<const WITH_PROBE: bool>() -> (Duration, u64) {
    let start = Instant::now();
    let speed = black_box(run::<WITH_PROBE>(black_box(50.0)));
    (start.elapsed(), speed.to_bits())
}

fn main() {
    black_box(measure::<false>());
    black_box(measure::<true>());

    let mut baseline = [Duration::ZERO; RUNS];
    let mut no_probe = [Duration::ZERO; RUNS];
    for run in 0..RUNS {
        let ((plain_time, plain_bits), (probe_time, probe_bits)) = if run % 2 == 0 {
            (measure::<false>(), measure::<true>())
        } else {
            let probe = measure::<true>();
            (measure::<false>(), probe)
        };
        assert_eq!(plain_bits, probe_bits);
        baseline[run] = plain_time;
        no_probe[run] = probe_time;
        println!(
            "run {} | no probe {plain_time:?} | NoProbe {probe_time:?}",
            run + 1
        );
    }
    baseline.sort_unstable();
    no_probe.sort_unstable();
    let baseline = baseline[RUNS / 2];
    let no_probe = no_probe[RUNS / 2];
    let overhead = 100.0 * (no_probe.as_secs_f64() / baseline.as_secs_f64() - 1.0);
    println!(
        "median over {STEPS} steps | no probe {baseline:?} | NoProbe {no_probe:?} | {overhead:+.3}%"
    );
    assert!(
        no_probe.as_nanos() * 100 <= baseline.as_nanos() * 101,
        "NoProbe overhead exceeds 1%"
    );
}
