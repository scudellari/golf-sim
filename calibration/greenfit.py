"""Fit the loaded green impulse from the SPIN07, Haake and R04 training rows."""
import math

from bouncefit import INERTIA, RADIUS
from optimize import minimize
from targets import fit_measurements

QUANTITIES = ("out_speed_mps", "out_angle_deg", "out_spin_rad_s")


def fit_rows(targets, source):
    for row in targets.rows((source,), "turf_impact"):
        inputs, measured = row["inputs"], fit_measurements(row)
        if not measured or inputs["surface"] != "green":
            continue
        if source == "H" and inputs.get("green_class") != "slipping":
            continue
        firmness = {"SPIN07": 1.0, "H": 0.8}.get(source)
        if firmness is None:
            firmness = min(inputs["stiffness_mpa"] / 12.0, 1.0)
        # R04 and Haake supply comparison scales for the fit, with no stated scatter.
        tolerance = row["tolerance"] if source == "SPIN07" else row["check"]
        scale = "sd" if source == "SPIN07" else "abs"
        yield (
            (inputs["speed_mps"], inputs["angle_deg"], inputs["spin_rad_s"], firmness),
            tuple(measured[name] for name in QUANTITIES),
            tuple(tolerance[name][scale] for name in QUANTITIES),
        )


def rebound(parameters, tangential_cor, speed, angle, spin, firmness):
    (tilt_base, tilt_angle, tilt_speed, tilt_firmness,
     cor_base, cor_angle, cor_angle_squared, cor_firmness,
     friction_base, friction_angle, friction_firmness, friction_speed_squared) = parameters
    descent = (angle - 45.0) / 10.0
    pace = (speed - 22.86) / 5.0
    firm = firmness - 0.5
    tilt_deg = (tilt_base + tilt_angle * descent * (1.0 - firmness)
                + tilt_speed * pace - tilt_firmness * firm)
    tilt = math.radians(max(0.0, min(35.0, tilt_deg)))
    cor = cor_base + cor_angle * descent - cor_angle_squared * descent**2 + cor_firmness * firm
    cor = max(0.04, min(0.4, cor)) * (1.0 - 0.15 * 0.3)
    friction = (friction_base + friction_angle * descent + friction_firmness * firm
                + friction_speed_squared * pace**2)
    friction = max(0.01, min(0.43, friction))
    inlet = math.radians(angle) + tilt
    normal_speed = speed * math.sin(inlet)
    tangent_speed = speed * math.cos(inlet)
    limit = friction * (1.0 + cor) * normal_speed
    slip = tangent_speed + spin * RADIUS
    impulse = -(1.0 + tangential_cor) * slip * INERTIA / (1.0 + INERTIA)
    impulse = max(-limit, min(limit, impulse))
    normal_out, tangent_out = cor * normal_speed, tangent_speed + impulse
    vx = tangent_out * math.cos(tilt) - normal_out * math.sin(tilt)
    vy = tangent_out * math.sin(tilt) + normal_out * math.cos(tilt)
    keep = math.sqrt(math.exp(-0.55 * max(0.0, -firm)))
    return (math.hypot(vx, vy) * keep, math.degrees(math.atan2(vy, vx)),
            (spin + impulse / (INERTIA * RADIUS)) * keep)


def fit(targets, tangential_cor):
    sources = [(source, weight, list(fit_rows(targets, source)))
               for source, weight in (("SPIN07", 4), ("H", 2), ("R04", 3))]
    bounds = [(12, 27), (-18, -4), (0, 10), (2, 30),
              (0.05, 0.35), (-0.12, 0.12), (0, 0.08), (0, 0.3),
              (0.09, 0.28), (-0.07, 0), (0.055, 0.18), (-0.03, 0.10)]

    def objective(parameters):
        if any(not low <= value <= high for value, (low, high) in zip(parameters, bounds)):
            return 1e6
        total = 0.0
        for source, weight, rows in sources:
            error = 0.0
            for incoming, measured, scale in rows:
                predicted = rebound(parameters, tangential_cor, *incoming)
                for value, target, unit in zip(predicted, measured, scale, strict=True):
                    residual = abs(value - target) / unit
                    error += residual**2
                    if source == "SPIN07":
                        error += max(0.0, residual - 0.7)**2
            total += weight * error / len(rows)
        return total

    start = [22, -6, 3, 12, 0.14, 0.01, 0.015, 0.04, 0.19, -0.012, 0.07, 0.03]
    steps = [1, 0.5, 0.6, 2, 0.02, 0.01, 0.01, 0.04, 0.03, 0.01, 0.03, 0.015]
    best = minimize(objective, start, steps, iterations=3600)
    for _ in range(2):
        best = minimize(objective, best, [step / 3 for step in steps], iterations=2400)
    names = ("TILT_DEG", "TILT_ANGLE", "TILT_SPEED", "TILT_FIRMNESS",
             "COR", "COR_ANGLE", "COR_ANGLE_SQUARED", "COR_FIRMNESS",
             "FRICTION", "FRICTION_ANGLE", "FRICTION_FIRMNESS", "FRICTION_SPEED_SQUARED")
    return {f"GREEN_IRON_{name}": value for name, value in zip(names, best, strict=True)}
