# Calibration

From the repository root, run `python3 -B calibration/fit.py` with Python 3.11
or newer. The script builds `golf` offline in release mode. Set `GOLF` to a
prebuilt executable to use that build for verification.

`testdata/validation/local/`, the script reports the maximum Python versus
Rust flight errors and prints
`Reproduced AERO, IMPACT, BOUNCE, GREEN BOUNCE and RUNOUT.`

A public checkout prints these skip and reproduction lines alongside the
Biber fit summary.

```text
Skipped RUNOUT. Missing dataset local-runout (x10.json).
Reproduced BOUNCE and GREEN BOUNCE.
```

Both runs exit successfully when their checks pass. The local
`local-regression` copy is reserved for fitting. The current fit
uses the same inputs with or without that copy.

To verify the public mode in a checkout with the local files, run the following
from the repository root with no other validation jobs running:

```sh
(
	mkdir -p target
	scratch=$(mktemp -d target/calibration-check.XXXXXX) || exit
	mv testdata/validation/local "$scratch/local" || exit
	trap 'mv "$scratch/local" testdata/validation/local && rmdir "$scratch"' EXIT
	cargo test --workspace --offline -- --nocapture &&
		python3 -B calibration/fit.py
)
```

Use `--write` to update the fitted blocks after the candidate passes the
available checks. Keep local datasets in the ignored directory and out of commits.

The loaded green fit uses the SPIN07, slipping Haake and R04 sand-wedge training
rows. SPIN07 and Ganton have no TruFirm readings, so the fit uses nominal
firmness values of 1.0 and 0.8. R04 converts stiffness to the same 0–1 scale by
dividing MPa by 12. The two R04 ball covers share the standard ball model.
