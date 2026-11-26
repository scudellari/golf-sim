"""Calm, level-ground RK4 model for the aerodynamic fit.

The fit uses the production 60 Hz flight step. fit.py checks the rounded
result against the Rust integrator on the same launch rows.
"""
import math

from optimize import minimize
from targets import fit_measurements

MPH = 0.44704
YARD = 0.9144
RADIUS = 0.021335
MASS = 0.04593
SPIN_DECAY = 2e-5  # [A2], fixed during fitting.
RHO = (101325.0 - 0.5 * 3168.531) / (287.058 * 298.15) + (0.5 * 3168.531) / (461.495 * 298.15)
VISCOSITY = 1.716e-5 * (298.15 / 273.15) ** 1.5 * (273.15 + 110.4) / (298.15 + 110.4)
RE_PER_SPEED = RHO * 2.0 * RADIUS / VISCOSITY
DRAG_SCALE = 0.5 * RHO * math.pi * RADIUS**2 / MASS
GRAVITY = 9.80665

LAUNCH_KEYS = ("ball_mph", "launch_deg", "spin_rpm")
TOUR_METRICS = ("carry_yd", "apex_yd", "descent_deg")
COEFFICIENTS = ("CD_BASE", "CD_SPIN", "CD_SPIN2", "CD_CRISIS_RISE", "CL_SCALE", "CL_EXPONENT")


def launch(row):
    return tuple(row["inputs"][name] for name in LAUNCH_KEYS)


def fly(ball_mph, launch_degrees, rpm, coefficients, dt=1/60):
    base, linear, quadratic, crisis, lift, exponent = coefficients
    spin_peak = linear / (-2.0 * quadratic)
    speed = ball_mph * MPH
    launch = math.radians(launch_degrees)
    x, y = 0.0, 0.0
    vx, vy = speed * math.cos(launch), speed * math.sin(launch)
    spin = rpm * math.tau / 60.0
    apex = 0.0

    def acceleration(vx, vy):
        speed = math.hypot(vx, vy)
        ratio = RADIUS * spin / speed
        clipped = min(ratio, spin_peak)
        cd = base + linear * clipped + quadratic * clipped * clipped
        cd += crisis * max(0.0, min(1.0, (80000.0 - RE_PER_SPEED * speed) / 30000.0))
        cl = lift * ratio**exponent
        q = DRAG_SCALE * speed
        return q * (-cd * vx - cl * vy), q * (cl * vx - cd * vy) - GRAVITY

    half, sixth = dt / 2.0, dt / 6.0
    while True:
        ax, ay = acceleration(vx, vy)
        v2x, v2y = vx + ax * half, vy + ay * half
        a2x, a2y = acceleration(v2x, v2y)
        v3x, v3y = vx + a2x * half, vy + a2y * half
        a3x, a3y = acceleration(v3x, v3y)
        v4x, v4y = vx + a3x * dt, vy + a3y * dt
        a4x, a4y = acceleration(v4x, v4y)
        next_vx = vx + (ax + 2.0 * a2x + 2.0 * a3x + a4x) * sixth
        next_vy = vy + (ay + 2.0 * a2y + 2.0 * a3y + a4y) * sixth
        dx = (vx + 2.0 * v2x + 2.0 * v3x + v4x) * sixth
        dy = (vy + 2.0 * v2y + 2.0 * v3y + v4y) * sixth
        if vy > 0.0 >= next_vy:
            apex = y + vy * (vy / (vy - next_vy)) * half
        if y + dy <= 0.0:
            f = -y / dy
            land_vx = vx + (next_vx - vx) * f
            land_vy = vy + (next_vy - vy) * f
            spin *= math.exp(-SPIN_DECAY * (math.hypot(vx, vy) + math.hypot(land_vx, land_vy)) * half * f / RADIUS)
            return (x + dx * f) / YARD, apex / YARD, math.degrees(math.atan2(-land_vy, land_vx)), spin
        spin *= math.exp(-SPIN_DECAY * (math.hypot(vx, vy) + math.hypot(next_vx, next_vy)) * half / RADIUS)
        x, y = x + dx, y + dy
        vx, vy = next_vx, next_vy


def objective(coefficients, tour, gain_rows, baseline):
    bounds = [(0.14, 0.29), (0.15, 0.85), (-0.75, -0.15), (0.10, 0.40), (0.40, 0.65), (0.30, 0.75)]
    if any(not lo <= value <= hi for value, (lo, hi) in zip(coefficients, bounds)):
        return 1e6
    carry_error = height_error = angle_error = 0.0
    for row in tour:
        carry, apex, angle, _ = fly(*launch(row), coefficients)
        measured = fit_measurements(row)
        if "carry_yd" in measured:
            carry_error += (100.0 * (carry / measured["carry_yd"] - 1.0))**2
        if "apex_yd" in measured:
            height_error += (100.0 * (apex / measured["apex_yd"] - 1.0))**2
        if "descent_deg" in measured:
            angle_error += (angle - measured["descent_deg"])**2
    origin = fly(*launch(baseline), coefficients)[0]
    gain_error = sum((fly(*launch(row), coefficients)[0] - origin
                      - fit_measurements(row)["carry_gain_yd"])**2 for row in gain_rows)
    return (carry_error + 0.3 * height_error + 0.5 * angle_error) / len(tour) + gain_error / len(gain_rows)


def fit(targets):
    tour = [row for row in targets.rows(("X1", "X2"), "launch")
            if set(TOUR_METRICS).intersection(fit_measurements(row))]
    gain_rows = targets.rows(("X4",), "launch")
    baseline = next(row for row in gain_rows if row["id"] == "X4.attack-1.5.launch")
    gains = [row for row in gain_rows if "carry_gain_yd" in fit_measurements(row)]
    if not tour or not gains or not fit_measurements(baseline):
        raise ValueError("Flight fit requires launch rows and carry gains")
    start = [0.1831, 0.5959, -0.4992, 0.2589, 0.5099, 0.4526]
    best = minimize(lambda coefficients: objective(coefficients, tour, gains, baseline),
                    start, [0.008, 0.045, 0.045, 0.035, 0.03, 0.03], 240)
    best = [round(value, 5) for value in best]
    output = dict(zip(COEFFICIENTS, best))
    output["CD_SPIN_PEAK"] = round(-best[1] / (2.0 * best[2]), 5)
    return output
