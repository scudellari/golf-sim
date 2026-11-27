use golf_sim_base::{
    Real, Seed, Surface, Vec2, Vec3, corpus,
    digest::object,
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
    let ground = dict(field(input, "ground"));
    let air = air(dict(field(input, "air")));
    let cup = vector3(field(input, "cup"));
    let cup = cup.is_finite().then_some(cup);
    let clock = number(input, "clock_s");
    match field(ground, "kind").as_str().unwrap() {
        "course_ground" => {
            let course = course.expect("course resolved for course_ground");
            let ground = CourseGround::new(
                course,
                number(ground, "firmness"),
                number(ground, "moisture"),
                number(ground, "green_speed_m"),
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

pub(super) fn replay(case: &Dict, course: Option<&Course>) -> Variant {
    let input = dict(field(case, "input"));
    let world = world(dict(field(input, "world")), course);
    let spot = |name| vector3(field(input, name));
    match field(case, "call").as_str().unwrap() {
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
            let address = world.address_lie(
                ball_lie(dict(field(input, "ball_lie"))),
                &StanceFootprint {
                    lead_foot_m: offset("lead_foot_m"),
                    trail_foot_m: offset("trail_foot_m"),
                    club_entry_m: offset("club_entry_m"),
                },
                spot("aim"),
            );
            record! {
                ball: ball_snapshot(address.ball),
                slope_along_rad: address.slope_along_rad,
                slope_across_rad: address.slope_across_rad,
                ball_slope_along_rad: address.ball_slope_along_rad,
                lead_foot_surface: address.lead_foot_surface as i64,
                trail_foot_surface: address.trail_foot_surface as i64,
                club_entry_surface: address.club_entry_surface as i64,
            }
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
    }
}
