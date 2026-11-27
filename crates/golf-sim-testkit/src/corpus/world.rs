use golf_sim_base::{
    Real, Seed, Surface, Vec2, Vec3, corpus,
    digest::object,
    fm,
    trace::{NoProbe, Trace},
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{
    grounds::{CourseGround, PracticeGround},
    lies::{BallLie, DivotLie, Sit},
    wear::{BallMark, Divot, DivotState, Wear, WearMark, WearMarkKind},
    world::World,
};
use golf_sim_course::{course::Course, legal::StanceFootprint};
use golf_sim_physics::{
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample, GroundSampler},
    values::Zone,
};

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn field<'a>(input: &'a Dict, name: &str) -> &'a Variant {
    input.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().unwrap()
}

fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected an array");
    };
    &array.items
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(number) = value else {
        panic!("expected an integer");
    };
    number.value()
}

fn code(value: &Variant) -> usize {
    usize::try_from(integer(value)).unwrap()
}

fn number(input: &Dict, name: &str) -> Real {
    let Variant::Float(number) = field(input, name) else {
        panic!("expected a float at {name}");
    };
    number.value()
}

fn boolean(value: &Variant) -> bool {
    let Variant::Bool(value) = value else {
        panic!("expected a boolean");
    };
    *value
}

fn vector2(value: &Variant) -> Vec2 {
    let Variant::Vector2(vector) = value else {
        panic!("expected a Vector2");
    };
    let [x, y] = vector.values;
    Vec2::new(x, y)
}

fn vector3(value: &Variant) -> Vec3 {
    let Variant::Vector3(vector) = value else {
        panic!("expected a Vector3");
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}

fn vector(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}

fn array(values: impl IntoIterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(values.into_iter().collect()))
}

fn ground_sample(input: &Dict) -> GroundSample {
    GroundSample {
        surface: Surface::ALL[code(field(input, "surface"))],
        height_m: number(input, "height_m"),
        normal: vector3(field(input, "normal")),
        firmness: number(input, "firmness"),
        moisture: number(input, "moisture"),
        green_speed_m: number(input, "green_speed_m"),
        grass_height_m: number(input, "grass_height_m"),
        grass_density: number(input, "grass_density"),
    }
}

fn plane(input: &Dict) -> PlaneGround {
    assert_eq!(field(input, "kind").as_str(), Some("plane"));
    let mut ground = PlaneGround::new(
        ground_sample(dict(field(input, "base"))),
        vector3(field(input, "origin")),
    );
    for patch in items(field(input, "patches")).iter().map(dict) {
        ground.add_patch(
            vector2(field(patch, "min")),
            vector2(field(patch, "max")),
            ground_sample(dict(field(patch, "sample"))),
        );
    }
    for trunk in items(field(input, "trunks")).iter().map(dict) {
        ground.add_trunk(
            vector2(field(trunk, "c")),
            number(trunk, "r"),
            number(trunk, "h"),
        );
    }
    for canopy in items(field(input, "canopies")).iter().map(dict) {
        ground.add_canopy(
            vector3(field(canopy, "c")),
            number(canopy, "r"),
            number(canopy, "d"),
        );
    }
    if let Some(zones) = field(input, "zones").as_dict() {
        let kinds = [Zone::InPlay, Zone::PenaltyArea, Zone::OutOfBounds];
        ground.set_outside_zone(kinds[code(field(zones, "outside"))]);
        let Variant::PackedInt32Array(codes) = field(zones, "kinds") else {
            panic!("expected zone kinds");
        };
        let polygons = items(field(zones, "polygons"));
        assert_eq!(codes.len(), polygons.len());
        for (&kind, polygon) in codes.iter().zip(polygons) {
            let Variant::PackedVector2Array(polygon) = polygon else {
                panic!("expected a zone polygon");
            };
            let &[[x0, z0], lower_right, [x1, z1], upper_left] = polygon.values.as_slice() else {
                panic!("expected a rectangular zone");
            };
            assert_eq!(lower_right, [x1, z0]);
            assert_eq!(upper_left, [x0, z1]);
            ground.add_zone(
                Vec2::new(x0, z0),
                Vec2::new(x1, z1),
                kinds[usize::try_from(kind).unwrap()],
            );
        }
    }
    ground
}

fn air(input: &Dict) -> ConstantAir {
    assert_eq!(field(input, "kind").as_str(), Some("constant_air"));
    let sample = dict(field(input, "air"));
    let mut air = ConstantAir::new(AirSample {
        temperature_c: number(sample, "temperature_c"),
        pressure_pa: number(sample, "pressure_pa"),
        relative_humidity: number(sample, "relative_humidity"),
        mean_wind_mps: vector3(field(sample, "mean_wind_mps")),
        gust_mps: vector3(field(sample, "gust_mps")),
        wind_reference_height_m: number(sample, "wind_reference_height_m"),
        roughness_length_m: number(sample, "roughness_length_m"),
    });
    for gust in items(field(input, "gusts")).iter().map(dict) {
        air.add_gust(
            number(gust, "start"),
            number(gust, "end"),
            vector3(field(gust, "gust")),
        );
    }
    air
}

pub(super) fn world<'a>(input: &Dict, course: Option<&'a Course>) -> World<'a> {
    world_with_ground(input, course, None)
}

fn world_with_ground<'a>(
    input: &Dict,
    course: Option<&'a Course>,
    shared_ground: Option<&CourseGround<'a>>,
) -> World<'a> {
    let ground = dict(field(input, "ground"));
    let air = air(dict(field(input, "air")));
    let cup = vector3(field(input, "cup"));
    let cup = cup.is_finite().then_some(cup);
    let clock = number(input, "clock_s");
    match field(ground, "kind").as_str().unwrap() {
        "course_ground" => {
            let course = course.expect("course resolved for course_ground");
            let firmness = number(ground, "firmness");
            let moisture = number(ground, "moisture");
            let green_speed = number(ground, "green_speed_m");
            let ground = shared_ground.map_or_else(
                || CourseGround::new(course, firmness, moisture, green_speed),
                |ground| ground.with_turf(firmness, moisture, green_speed),
            );
            World::on_course(ground, air, cup, clock)
        }
        "practice_ground" => {
            let mut practice = PracticeGround::new(plane(dict(field(ground, "plane"))));
            let gradient = vector2(field(ground, "pad_gradient"));
            practice.tilt_pad(
                vector3(field(ground, "pad_centre")),
                Vec3::new(gradient.x, 0.0, gradient.y),
                gradient.length(),
                number(ground, "flat_radius_m"),
                number(ground, "blend_radius_m"),
            );
            World::practice(
                practice,
                air,
                ball_lie(dict(field(input, "lie"))),
                cup,
                clock,
            )
        }
        "plane" => World::practice(
            plane(ground),
            air,
            ball_lie(dict(field(input, "lie"))),
            cup,
            clock,
        ),
        kind => panic!("unknown ground {kind}"),
    }
}

fn ball_lie(input: &Dict) -> BallLie {
    BallLie {
        spot: vector3(field(input, "spot")),
        surface: Surface::ALL[code(field(input, "surface"))],
        sit: [Sit::Up, Sit::Normal, Sit::Down][code(field(input, "sit"))],
        ball_height_m: number(input, "ball_height_m"),
        grass_height_m: number(input, "grass_height_m"),
        grass_density: number(input, "grass_density"),
        sand_firmness: number(input, "sand_firmness"),
        divot: [
            DivotLie::None,
            DivotLie::Fresh,
            DivotLie::Old,
            DivotLie::SandFilled,
        ][code(field(input, "divot"))],
    }
}

fn ball_snapshot(ball: BallLie) -> Variant {
    record! {
        spot: vector(ball.spot),
        surface: ball.surface as i64,
        sit: ball.sit as i64,
        ball_height_m: ball.ball_height_m,
        grass_height_m: ball.grass_height_m,
        grass_density: ball.grass_density,
        sand_firmness: ball.sand_firmness,
        divot: ball.divot as i64,
    }
}

fn wear(input: &Dict) -> Wear {
    Wear::new(
        items(field(input, "divots"))
            .iter()
            .map(dict)
            .map(|divot| Divot {
                hole: integer(field(divot, "hole")),
                position: vector3(field(divot, "position")),
                bearing_rad: number(divot, "bearing_rad"),
                length_m: number(divot, "length_m"),
                width_m: number(divot, "width_m"),
                depth_m: number(divot, "depth_m"),
                state: [DivotState::Fresh, DivotState::Old, DivotState::SandFilled]
                    [code(field(divot, "state"))],
                own: boolean(field(divot, "own")),
            })
            .collect(),
        items(field(input, "ball_marks"))
            .iter()
            .map(dict)
            .map(|mark| BallMark {
                hole: integer(field(mark, "hole")),
                position: vector3(field(mark, "position")),
                depth_m: number(mark, "depth_m"),
                own: boolean(field(mark, "own")),
            })
            .collect(),
    )
}

fn divot_snapshot(divot: &Divot) -> Variant {
    record! {
        hole: divot.hole,
        position: vector(divot.position),
        bearing_rad: divot.bearing_rad,
        length_m: divot.length_m,
        width_m: divot.width_m,
        depth_m: divot.depth_m,
        state: divot.state as i64,
        own: Variant::Bool(divot.own),
    }
}

const SINGLE_STEP: Real = f32::EPSILON as Real;

fn round_vector(point: Vec3) -> Vec3 {
    Vec3::new(
        Real::from(point.x as f32),
        Real::from(point.y as f32),
        Real::from(point.z as f32),
    )
}

fn rounded_stance_point(ball: Vec3, aim: Vec3, offset: Vec2) -> Vec3 {
    let forward = round_vector(Vec3::new(aim.x, 0.0, aim.z).normalized());
    let right = forward.cross(Vec3::new(0.0, 1.0, 0.0));
    let first = round_vector(ball + round_vector(forward * offset.x));
    round_vector(first + round_vector(right * offset.y))
}

fn stance_height_error(course: &Course, ball: Vec3, aim: Vec3, offset: Vec2) -> Real {
    let rounded = rounded_stance_point(ball, aim, offset);
    let precise = StanceFootprint::to_world(offset, ball, aim);
    let split = |p: Vec3| course.ground_split_at(Vec2::new(p.x, p.z));
    let packed = split(rounded);
    let exact = split(precise).height();
    // The reference passes both the interpolation nodes and the split height
    // through packed vectors. The stencil's weights have L1 norm at most
    // 1.25 per axis. The offset carries the rounding of the weighted sums.
    let height_error =
        SINGLE_STEP * (1.25 * 1.25 * packed.datum_m.abs().max(1.0) + 8.0 * packed.offset_m.abs());
    (packed.height() - exact).abs() + height_error
}

fn slope_rounding_bound(height_error_m: Real, span_m: Real, angle: Real) -> Real {
    let grade = fm::tan(angle);
    let error = height_error_m / span_m.abs() + 4.0 * SINGLE_STEP * grade.abs();
    (fm::atan(grade + error) - angle).max(angle - fm::atan(grade - error))
}

fn exact_stance_slope(
    ball: Vec3,
    aim: Vec3,
    [a, b]: [Vec2; 2],
    span: Real,
    height_at: impl Fn(Vec2) -> Real,
) -> Real {
    if span.abs() <= 0.05 {
        return 0.0;
    }
    let forward = Vec2::new(aim.x, aim.z).normalized();
    let right = Vec2::new(-forward.y, forward.x);
    let [high, low] = [a, b].map(|offset| {
        height_at(Vec2::new(
            ball.x + forward.x * offset.x + right.x * offset.y,
            ball.z + forward.y * offset.x + right.y * offset.y,
        ))
    });
    fm::atan((high - low) / span)
}

fn matches_stance_slope(computed: Real, exact: Real) -> bool {
    (computed - exact).abs() <= 1e-11
}

fn compare_stance(
    case: &Dict,
    course: &Course,
    ball: Vec3,
    aim: Vec3,
    footprint: &StanceFootprint,
    output: &mut Variant,
    drift_report: bool,
) -> Result<(), String> {
    let expected = dict(field(case, "output"));
    let Variant::Dict(actual) = output else {
        unreachable!("address lie output");
    };
    let centre = footprint.centre_m();
    for (key, a, b, span) in [
        (
            "slope_along_rad",
            footprint.lead_foot_m,
            footprint.trail_foot_m,
            footprint.lead_foot_m.x - footprint.trail_foot_m.x,
        ),
        (
            "slope_across_rad",
            Vec2::new(centre.x, 0.0),
            centre,
            -centre.y,
        ),
    ] {
        let recorded = number(expected, key);
        let computed = number(actual, key);
        let exact = exact_stance_slope(ball, aim, [a, b], span, |point| {
            course.ground_split_at(point).height()
        });
        if !matches_stance_slope(computed, exact) {
            return Err(format!(
                "{key}: core {computed:.e} differs from sampled grade {exact:.e}"
            ));
        }
        let error = (recorded - computed).abs();
        if error <= 1e-6_f64.max(1e-5 * recorded.abs().max(computed.abs())) || span.abs() <= 0.05 {
            continue;
        }
        let heights =
            stance_height_error(course, ball, aim, a) + stance_height_error(course, ball, aim, b);
        let bound = slope_rounding_bound(heights, span, exact);
        let id = field(case, "id").as_str().unwrap();
        println!(
            "case {id}: {key} drift {error:.3e} rad, stance placement and height rounding bound {bound:.3e} rad"
        );
        if !drift_report && error <= bound {
            *actual.get_mut(key).unwrap() = field(expected, key).clone();
        }
    }
    Ok(())
}

pub(super) fn replay(
    case: &Dict,
    course: Option<&Course>,
    shared_ground: Option<&CourseGround<'_>>,
    drift_report: bool,
) -> Result<Variant, String> {
    let input = dict(field(case, "input"));
    let world_input = dict(field(input, "world"));
    let world = world_with_ground(world_input, course, shared_ground);
    let spot = |name| vector3(field(input, name));
    Ok(match field(case, "call").as_str().unwrap() {
        "course_ground.sample_ground" | "practice_ground.sample_ground" => {
            let position = spot("position");
            let sample = world.ground().sample_ground(position.x, position.z);
            record! {
                surface: sample.surface as i64,
                height_m: sample.height_m,
                normal: vector(sample.normal),
                firmness: sample.firmness,
                moisture: sample.moisture,
                green_speed_m: sample.green_speed_m,
                grass_height_m: sample.grass_height_m,
                grass_density: sample.grass_density,
            }
        }
        "world.settle" => {
            let seed = Seed(integer(field(input, "seed_value")));
            let spot = spot("spot");
            let without_trace = ball_snapshot(world.settle(spot, seed, &mut NoProbe));
            let traced = ball_snapshot(world.settle(spot, seed, &mut Trace::default()));
            assert_eq!(corpus::first_difference(&without_trace, &traced), None);
            without_trace
        }
        "world.address_lie" => {
            let footprint = dict(field(input, "footprint"));
            let offset = |name| vector2(field(footprint, name));
            let footprint = StanceFootprint {
                lead_foot_m: offset("lead_foot_m"),
                trail_foot_m: offset("trail_foot_m"),
                club_entry_m: offset("club_entry_m"),
            };
            let ball = ball_lie(dict(field(input, "ball_lie")));
            let aim = spot("aim");
            let address = world.address_lie(ball, &footprint, aim);
            let mut output = record! {
                ball: ball_snapshot(address.ball),
                slope_along_rad: address.slope_along_rad,
                slope_across_rad: address.slope_across_rad,
                ball_slope_along_rad: address.ball_slope_along_rad,
                lead_foot_surface: address.lead_foot_surface as i64,
                trail_foot_surface: address.trail_foot_surface as i64,
                club_entry_surface: address.club_entry_surface as i64,
            };
            if let Some(course) = course
                && field(dict(field(world_input, "ground")), "kind").as_str()
                    == Some("course_ground")
            {
                compare_stance(
                    case,
                    course,
                    ball.spot,
                    aim,
                    &footprint,
                    &mut output,
                    drift_report,
                )?;
            }
            output
        }
        "world.wind_readout" => {
            let wind = world.wind_readout(spot("spot"), number(input, "gust_time_s"));
            record! {
                mean_mps: vector(wind.mean_mps),
                gust_mps: vector(wind.gust_mps),
                ground_mps: vector(wind.ground_mps),
                felt_mps: vector(wind.felt_mps),
                under_trees: Variant::Bool(wind.under_trees),
                temperature_c: wind.temperature_c,
                pressure_pa: wind.pressure_pa,
                relative_humidity: wind.relative_humidity,
            }
        }
        "world.wind_at_heights" => {
            let Variant::PackedFloat32Array(heights) = field(input, "heights_m") else {
                panic!("expected packed heights");
            };
            let wind = world.wind_at_heights(spot("spot"), number(input, "gust_time_s"), heights);
            Variant::PackedVector3Array(Components {
                values: wind.iter().map(|v| [v.x, v.y, v.z]).collect(),
                width: FloatWidth::Double,
            })
        }
        "wear.divot_at" => wear(dict(field(input, "wear")))
            .divot_at(spot("spot"))
            .map_or(Variant::Nil, divot_snapshot),
        "wear.with_marks" => {
            let marks: Vec<_> = items(field(input, "marks"))
                .iter()
                .map(dict)
                .map(|mark| WearMark {
                    kind: [WearMarkKind::Divot, WearMarkKind::PitchMark][code(field(mark, "kind"))],
                    start: vector3(field(mark, "start")),
                    end: vector3(field(mark, "end")),
                    depth_m: number(mark, "depth_m"),
                    surface: Surface::ALL[code(field(mark, "surface"))],
                })
                .collect();
            let next =
                wear(dict(field(input, "wear"))).with_marks(&marks, integer(field(input, "hole")));
            record! {
                divots: array(next.divots().iter().map(divot_snapshot)),
                ball_marks: array(next.ball_marks().iter().map(|mark| record! {
                    hole: mark.hole,
                    position: vector(mark.position),
                    depth_m: mark.depth_m,
                    own: Variant::Bool(mark.own),
                })),
            }
        }
        call => panic!("unknown call {call}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rounding_envelope_cannot_hide_an_incorrect_grade() {
        let exact = exact_stance_slope(
            Vec3::new(100.0, 12.0, 200.0),
            Vec3::new(1.0, 0.0, 0.0),
            [Vec2::new(0.4, 0.0), Vec2::new(-0.4, 0.0)],
            0.8,
            |p| 0.25 * p.x + 0.1 * p.y,
        );
        assert!(matches_stance_slope(fm::atan(0.25), exact));
        let biased = exact + 1e-5;
        assert!((biased - exact).abs() < slope_rounding_bound(1e-4, 0.8, exact));
        assert!(!matches_stance_slope(biased, exact));
    }

    #[test]
    fn stance_height_rounding_scales_with_position_and_span() {
        let ball = Vec3::new(2048.0, 0.0, 0.0);
        let aim = Vec3::new(1.0, 0.0, 0.0);
        let lead = Vec2::new(0.17, 0.0);
        let trail = Vec2::new(-0.17, 0.0);
        let height = |point: Vec3| (point.x - ball.x) * 0.1;
        let mut height_error = 0.0;
        let mut gap = 0.0;
        for (offset, sign) in [(lead, 1.0), (trail, -1.0)] {
            let exact = height(StanceFootprint::to_world(offset, ball, aim));
            let rounded = height(rounded_stance_point(ball, aim, offset));
            height_error += (exact - rounded).abs();
            gap += sign * rounded;
        }
        let span = lead.x - trail.x;
        let exact_angle = fm::atan(0.1);
        let drift = (fm::atan(gap / span) - exact_angle).abs();
        assert!(drift > 1e-6);
        let bound = slope_rounding_bound(height_error, span, exact_angle);
        assert!(drift <= bound);
        assert!(bound < 1e-4);
    }
}
