"""Small deterministic Nelder–Mead minimizer for the calibration scripts."""


def minimize(objective, start, steps, iterations=400):
    size = len(start)
    simplex = [list(start)]
    for axis, step in enumerate(steps):
        point = list(start)
        point[axis] += step
        simplex.append(point)
    scored = [(objective(point), point) for point in simplex]

    for _ in range(iterations):
        scored.sort()
        best_value, best = scored[0]
        worst_value, worst = scored[-1]
        if worst_value - best_value < 1e-10:
            break
        centre = [sum(point[i] for _, point in scored[:-1]) / size for i in range(size)]
        reflected = [2.0 * c - w for c, w in zip(centre, worst)]
        reflected_value = objective(reflected)
        if reflected_value < best_value:
            expanded = [c + 2.0 * (r - c) for c, r in zip(centre, reflected)]
            scored[-1] = min((reflected_value, reflected), (objective(expanded), expanded))
        elif reflected_value < scored[-2][0]:
            scored[-1] = (reflected_value, reflected)
        else:
            endpoint = reflected if reflected_value < worst_value else worst
            contracted = [(c + e) * 0.5 for c, e in zip(centre, endpoint)]
            contracted_value = objective(contracted)
            if contracted_value < min(reflected_value, worst_value):
                scored[-1] = (contracted_value, contracted)
            else:
                for i, (_, point) in enumerate(scored[1:], 1):
                    shrunk = [(b + p) * 0.5 for b, p in zip(best, point)]
                    scored[i] = (objective(shrunk), shrunk)
    return min(scored)[1]
