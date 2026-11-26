# Calibration

From the repository root, run `python3 -B calibration/fit.py` with Python 3.11
or newer. The script builds `golf` offline in release mode. Set `GOLF` to a
prebuilt executable to use that build for verification.

With the `local-launches` files listed in the licence ledger under
`testdata/validation/local/`, expect
`Reproduced AERO, IMPACT, BOUNCE, GREEN BOUNCE and RUNOUT.`
In a public checkout, expect `Skipped AERO and IMPACT. Missing dataset
local-launches (x1.json, x2.json).` followed by
`Reproduced BOUNCE, GREEN BOUNCE and RUNOUT.`
Both runs check Python flight parity against Rust on the available launch rows
and exit successfully when their checks pass.

To verify the public mode in a checkout with the local files, run the following
from the repository root with no other validation jobs running:

```sh
(
    mkdir -p target
    scratch=$(mktemp -d target/calibration-check.XXXXXX) || exit
    mv testdata/validation/local "$scratch/local" || exit
    trap 'mv "$scratch/local" testdata/validation/local && rmdir "$scratch"' EXIT
    python3 -B calibration/fit.py
)
```

Use `--write` to update the fitted blocks after the candidate passes the flight
parity check. Keep local datasets in the ignored directory and out of commits.

The loaded green fit uses the SPIN07, slipping Haake and R04 sand-wedge training
rows. SPIN07 and Ganton have no TruFirm readings, so the fit uses nominal
firmness values of 1.0 and 0.8. R04 converts stiffness to the same 0–1 scale by
dividing MPa by 12. The two R04 ball covers share the standard ball model.
