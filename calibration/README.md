# Reproduce the physics fit

From the workspace root, run the script with Python 3.11 or later.

```sh
GOLF_SIM_LOCAL_DATA= python3 -B calibration/fit.py
```

The public run checks the two bounce blocks and prints:

```text
Skipped IMPACT. No targets with the impact role.
Skipped AERO. No targets with the aero role.
B fit (543), RMS m/s (vx, vy, rω): 1.422 0.972 1.836
B held out (150), RMS m/s (vx, vy, rω): 1.443 1.3 2.19
A artificial turf (330), RMS m/s (vx, vy, rω): 3.814 5.199 3.645
Skipped RUNOUT. No targets with the runout role.
Skipped flight parity. No targets with the parity role.
Reproduced BOUNCE and GREEN BOUNCE.
```

The script exits successfully when all available checks pass. It uses `cargo run --offline` to build and run `golf`. Set `GOLF` to a prebuilt executable to reuse it.

Use `--write` to update fitted constants. The script builds the candidate before writing it and checks flight parity when it has launch inputs. See [Contributing](../CONTRIBUTING.md#validation-data-and-fixtures) for validation and fixture setup.
