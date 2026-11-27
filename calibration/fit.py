"""Reproduce the fitted constants in the Rust physics model.

Run with Python 3.11 or later. GOLF can name a prebuilt golf executable.
The default checks the committed blocks using the available datasets.
--write checks available flight launches with a scratch build before applying the fit.
"""
import argparse
import difflib
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from bouncefit import fit as fit_bounce
from flightfit import COEFFICIENTS, fit as fit_flight, fly, launch
from greenfit import fit as fit_green
from targets import ROOT, Targets

TUNABLES = Path("crates/golf-sim-physics/src/tunables.rs")
LOCAL_LAUNCHES = "local-launches"
RUNOUT = "local-runout"
CARGO = ("cargo", "run", "--offline", "--release", "-p", "golf-sim-cli", "--")
CLUBS = dict(zip(
    ("driver", "3wood", "hybrid", "4iron", "5iron", "6iron", "7iron", "8iron", "9iron", "pw"),
    ("Driver", "Wood3", "Hybrid", "Iron4", "Iron5", "Iron6", "Iron7", "Iron8", "Iron9", "PitchingWedge"),
))
PARITY_TOLERANCES = (0.12, 0.07, 0.06, 0.6)


def invoke(command, targets, root, job):
    env = os.environ.copy()
    target_dir = ROOT / "target"
    env["CARGO_TARGET_DIR"] = str(target_dir if root == ROOT else target_dir / "calibration")
    result = subprocess.run(
        [*command, "fit", job, "--data", str(targets.directory)], env=env, cwd=root,
        check=True, stdout=subprocess.PIPE, text=True,
    )
    return result.stdout.splitlines()


def records(lines, prefix):
    return [json.loads(line.removeprefix(prefix)) for line in lines if line.startswith(prefix)]


def format_constants(values):
    return "".join(f"pub const {name}: Real = {value:.5f};\n" for name, value in values.items())


def block_parts(text, name):
    start = f"// BEGIN FIT {name}\n"
    end = f"// END FIT {name}\n"
    left, rest = text.split(start, 1)
    body, right = rest.split(end, 1)
    return left + start, body, end + right


def replace_block(text, name, body):
    left, _, right = block_parts(text, name)
    return left + body + right


def impact_block(original, lines):
    fits = records(lines, "FIT ")
    values = dict(fits)
    if len(fits) != len(CLUBS) or values.keys() != CLUBS.keys():
        raise ValueError(f"Incomplete or unexpected impact fit clubs: {', '.join(sorted(values))}")
    [iron_slope] = records(lines, "IRON COR ")
    _, block, _ = block_parts(original, "IMPACT")
    for club, fitted in values.items():
        prefix = f"    (ClubId::{CLUBS[club]}, "
        line = f"{prefix}[{fitted[0]:.5f}, {fitted[1]:.5f}]),"
        block, count = re.subn(rf"^{re.escape(prefix)}.*$", line, block, flags=re.M)
        if count != 1:
            raise ValueError(f"Expected one impact fit entry for {club}")
    _, _, moi, slope = values["driver"]
    constants = {
        "IRON_COR_SPEED_SLOPE_DELTA": f"{iron_slope:.5f}",
        "DRIVER_MOI_HEEL_TOE": f"{moi:.1f}",
        "DRIVER_TANGENTIAL_SPEED_SLOPE": f"{slope:.5f}",
    }
    for name, value in constants.items():
        block, count = re.subn(rf"^pub const {name}: Real = .*;$",
                               f"pub const {name}: Real = {value};", block, flags=re.M)
        if count != 1:
            raise ValueError(f"Expected one constant for {name} in the impact block")
    return block


def checked_aero(text):
    _, block, _ = block_parts(text, "AERO")
    values = dict(re.findall(r"^pub const (\w+): Real = ([^;]+);$", block, re.M))
    return [float(values[name]) for name in COEFFICIENTS]


def can_fit(targets, name, dataset, *files):
    missing = [file for file in targets.missing_local_files(dataset) if file in files]
    if missing:
        print(f"Skipped {name}. Missing dataset {dataset} ({', '.join(missing)}).", flush=True)
    return not missing


def runout_block(command, targets, root, text):
    [stimp] = records(invoke(command, targets, root, "runout"), "RUNOUT ")
    return replace_block(text, "RUNOUT", format_constants({"FAIRWAY_STIMP_M": stimp}))


def check_parity(command, targets, root, text):
    launch_files = ("x1.json", "x2.json", "x4.json", "x9.json")
    missing = targets.missing_local_files(LOCAL_LAUNCHES)
    if all(file in missing for file in launch_files):
        can_fit(targets, "flight parity", LOCAL_LAUNCHES, *launch_files)
        return
    rows = targets.rows(("X1", "X2", "X4", "X9"), "launch")
    actual = records(invoke(command, targets, root, "parity"), "FLIGHT ")
    if not rows or len(actual) != len(rows):
        raise ValueError(f"Rust returned {len(actual)} flight rows, expected {len(rows)}")
    coefficients = checked_aero(text)
    maxima = [0.0] * 4
    for row, measured in zip(rows, actual, strict=True):
        predicted = fly(*launch(row), coefficients)
        maxima = [max(m, abs(x - y)) for m, x, y in zip(maxima, measured, predicted, strict=True)]
    print(f"Python versus Rust max errors for {len(rows)} launches "
          f"(carry yd, apex yd, descent deg, landing spin rad/s): {maxima}", flush=True)
    if any(error > limit for error, limit in zip(maxima, PARITY_TOLERANCES)):
        raise ValueError(f"Flight solver parity exceeded {PARITY_TOLERANCES}")


def show_diff(original, candidate):
    print("".join(difflib.unified_diff(original.splitlines(keepends=True),
          candidate.splitlines(keepends=True), "checked in", "reproduced")), end="", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    golf = os.environ.get("GOLF")
    command = [shutil.which(golf) or str(Path(golf).resolve())] if golf else CARGO
    tunables = ROOT / TUNABLES
    original = tunables.read_text(encoding="utf-8")

    with tempfile.TemporaryDirectory(prefix="golf-calibration-") as directory:
        scratch = Path(directory)
        targets = Targets(scratch)
        candidate = original
        reproduced = []
        has_impact = can_fit(targets, "IMPACT", LOCAL_LAUNCHES, "x1.json")
        if has_impact:
            impact = impact_block(original, invoke(command, targets, ROOT, "impact"))
            candidate = replace_block(candidate, "IMPACT", impact)
        if can_fit(targets, "AERO", LOCAL_LAUNCHES, "x1.json", "x2.json", "x4.json"):
            candidate = replace_block(candidate, "AERO", format_constants(fit_flight(targets)))
            reproduced.append("AERO")
        if has_impact:
            reproduced.append("IMPACT")
        bounce = fit_bounce(targets)
        candidate = replace_block(candidate, "BOUNCE", format_constants(bounce))
        candidate = replace_block(candidate, "GREEN BOUNCE", format_constants(fit_green(
            targets, bounce["BOUNCE_GREEN_TANGENTIAL_COR"],
        )))
        reproduced.extend(("BOUNCE", "GREEN BOUNCE"))

        has_runout = can_fit(targets, "RUNOUT", RUNOUT, "x10.json")
        generated = candidate
        if has_runout:
            generated = runout_block(command, targets, ROOT, candidate)
            reproduced.append("RUNOUT")
        if generated != original and not args.write:
            show_diff(original, generated)
            return 1

        if args.write and generated != original:
            # Bounce and flight affect the run-out solve. Build the candidate away from
            # the working tree so a failed build or parity check leaves the tunables intact.
            staged = scratch / "candidate"
            shutil.copytree(ROOT, staged, ignore=shutil.ignore_patterns(".git", "target", "local"))
            staged_tunables = staged / TUNABLES
            staged_tunables.write_text(candidate, encoding="utf-8")
            if has_runout:
                generated = runout_block(CARGO, targets, staged, candidate)
            check_parity(CARGO, targets, staged, generated)
            show_diff(original, generated)
            tunables.write_text(generated, encoding="utf-8")
            print("Calibration blocks updated.")
        else:
            check_parity(command, targets, ROOT, original)
            print(f"Reproduced {', '.join(reproduced[:-1])} and {reproduced[-1]}.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Calibration failed: {error}") from None
