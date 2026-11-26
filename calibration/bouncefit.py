"""Fit a continuous tilted-plane impulse to Biber's natural-turf campaign.

The target roles assign the natural-turf training and held-out sets.
Campaign A is artificial turf and is reported for comparison. Spin errors
use rim speed.
"""
import csv
import math

from optimize import minimize
from targets import fit_measurements

RADIUS = 0.021335
INERTIA = 0.38


def campaign(targets, name):
    file = f"data_campaign{name}.csv"
    path = next(path for path in targets.files("biber-2023") if path.name == file)
    roles = {int(row["id"].rsplit(".", 1)[1]): fit_measurements(row)
             for row in targets.rows(("BIB",), "turf_impact")
             if row["id"].startswith(f"BIB.{name}.")}
    with path.open(newline="", encoding="utf-8") as source:
        data = [tuple(float(x) * RADIUS for x in row) for row in csv.reader(source)]
    if roles.keys() != set(range(1, len(data) + 1)):
        raise ValueError(f"Biber target IDs do not cover the rows of {file}")
    quantities = {"out_vx_mps", "out_vy_mps", "out_spin_rad_s"}
    if any(role and role.keys() != quantities for role in roles.values()):
        raise ValueError(f"{file} has a partial fit row")
    return [(row, bool(roles[index])) for index, row in enumerate(data, 1)]


def rebound(params, vx, vy, wr):
    crater_scale, cap_deg, cor_floor, friction, tangential_cor = params
    speed = math.hypot(vx, vy)
    incidence = math.atan2(vx, -vy)
    penner_deg = 15.4 * speed / 18.6 * math.degrees(incidence) / 44.4
    load = max(0.0, min(1.0, (-vy - 0.5) / (3.0 - 0.5)))
    tilt = math.radians(min(cap_deg, crater_scale * penner_deg)) * load**2 * (3.0 - 2.0 * load)
    c, s = math.cos(tilt), math.sin(tilt)
    normal_speed = vx * s - vy * c
    normal_cor = 0.120
    if normal_speed <= 20.0:
        normal_cor = 0.510 - 0.0375 * normal_speed + 0.000903 * normal_speed**2
    normal_cor = max(normal_cor, cor_floor) * (1.0 - 0.15 * 0.3)
    jn = (1.0 + normal_cor) * normal_speed
    slip = vx * c + vy * s + wr
    jt = -(1 + tangential_cor) * slip * INERTIA / (1.0 + INERTIA)
    jt = max(-friction * jn, min(friction * jn, jt))
    return vx - s * jn + c * jt, vy + c * jn + s * jt, wr + jt / INERTIA


def errors(params, rows):
    sums = [0.0, 0.0, 0.0]
    for row in rows:
        for i, model in enumerate(rebound(params, *row[:3])):
            sums[i] += (model - row[3 + i]) ** 2
    return [math.sqrt(s / len(rows)) for s in sums]


def fit(targets):
    rows = campaign(targets, "B")
    train = [row for row, fit_role in rows if fit_role]
    holdout = [row for row, fit_role in rows if not fit_role]

    def objective(params):
        bounds = [(0.0, 2.0), (1.0, 35.0), (0.01, 1.2), (-0.25, 1.0)]
        if any(not lo <= p <= hi for p, (lo, hi) in zip(params, bounds)):
            return 1e6
        return sum(error**2 for error in errors([*params[:2], 0.0, *params[2:]], train))

    best = minimize(objective, [0.8, 18.0, 0.6, 0.15], [0.1, 2.0, 0.1, 0.1])
    best = [round(p, 5) for p in best]
    names = ["BOUNCE_CRATER_SCALE", "BOUNCE_CRATER_MAX_DEG",
             "BOUNCE_TURF_FRICTION", "BOUNCE_TANGENTIAL_COR"]
    values = dict(zip(names, best))
    # The two unclassified Haake impacts anchor the rolling response.
    haake = []
    for row in targets.rows(("H",), "turf_impact"):
        inputs, measured = row["inputs"], fit_measurements(row)
        if "green_class" in inputs or not measured:
            continue
        v, a, w = (inputs[key] for key in ("speed_mps", "angle_deg", "spin_rad_s"))
        u, b, q = (measured[key] for key in ("out_speed_mps", "out_angle_deg", "out_spin_rad_s"))
        a, b = map(math.radians, (a, b))
        haake.append((v * math.cos(a), -v * math.sin(a), w * RADIUS,
                      u * math.cos(b), u * math.sin(b), q * RADIUS))

    def green_objective(parameters):
        cap, floor, tangential = parameters
        if not (5 <= cap <= 25 and 0.13 <= floor <= 0.5 and 0.0 <= tangential <= 1.0):
            return 1e6
        return sum(e**2 for e in errors([1.4, cap, floor, 0.43, tangential], haake))

    green = minimize(green_objective, [14.0, 0.25, 0.15], [1.0, 0.02, 0.05])
    names = ["BOUNCE_GREEN_MAX_DEG", "BOUNCE_GREEN_COR_FLOOR", "BOUNCE_GREEN_TANGENTIAL_COR"]
    values.update(zip(names, (round(p, 5) for p in green)))
    artificial = [row for row, _ in campaign(targets, "A")]
    for label, data in [("B fit", train), ("B held out", holdout), ("A artificial turf", artificial)]:
        if data:
            rms = errors([*best[:2], 0.0, *best[2:]], data)
            print(f"{label} ({len(data)}), RMS m/s (vx, vy, rω):", *(round(e, 3) for e in rms))
    return values
