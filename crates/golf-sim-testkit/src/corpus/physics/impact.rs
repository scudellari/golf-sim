use std::cell::Cell;

use golf_sim_base::{
    Real, Surface, Vec2, Vec3, fm,
    trace::NoProbe,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    impact,
    plane::PlaneGround,
    samplers::{AirSample, GroundSample, GroundSampler},
    strike,
    values::{BallSpec, ClubCategory, ClubSpec, Delivery, Launch, Lie, ShotRequest},
};

pub const EIGHT_SINGLE_PRECISION_STEPS: Real = 8.0 / (1 << 23) as Real;

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected dictionary")
}

fn field<'a>(record: &'a Dict, key: &str) -> &'a Variant {
    record.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn array(value: &Variant) -> &[Variant] {
    let Variant::Array(value) = value else {
        panic!("expected array");
    };
    &value.items
}

fn integer(record: &Dict, key: &str) -> usize {
    let Variant::Int(value) = field(record, key) else {
        panic!("expected integer at {key}");
    };
    value.value().try_into().unwrap()
}

fn number(record: &Dict, key: &str) -> Real {
    match field(record, key) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected number at {key}"),
    }
}

fn vec2(record: &Dict, key: &str) -> Vec2 {
    let Variant::Vector2(v) = field(record, key) else {
        panic!("expected Vector2 at {key}");
    };
    let [x, y] = v.values;
    Vec2::new(x, y)
}

fn vec3(record: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(v) = field(record, key) else {
        panic!("expected Vector3 at {key}");
    };
    let [x, y, z] = v.values;
    Vec3::new(x, y, z)
}

fn vector(v: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [v.x, v.y, v.z],
        width: FloatWidth::Double,
    })
}

fn turf_point(point: Option<Vec3>) -> Variant {
    vector(point.unwrap_or(Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY)))
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        Variant::Dict(Dict::new(vec![$((stringify!($key).into(), ($value).into())),+]))
    };
}

macro_rules! read_fields {
    ($record:expr, $type:ident, $($key:ident),+ $(,)?) => {
        $type { $($key: number($record, stringify!($key))),+ }
    };
    ($record:expr, $type:ident { $($name:ident: $other:expr),+ }, $($key:ident),+ $(,)?) => {
        $type { $($name: $other,)+ $($key: number($record, stringify!($key))),+ }
    };
}

fn club(input: &Dict) -> ClubSpec {
    let category = [
        ClubCategory::Driver,
        ClubCategory::Wood,
        ClubCategory::Hybrid,
        ClubCategory::Iron,
        ClubCategory::Wedge,
        ClubCategory::Putter,
    ][integer(input, "category")];
    read_fields!(
        input,
        ClubSpec {
            id: field(input, "id").as_str().unwrap().to_owned(),
            category: category
        },
        loft_rad,
        length_m,
        head_mass_kg,
        cor,
        cor_speed_slope_delta,
        cor_falloff_toe,
        cor_falloff_up,
        cg_depth_m,
        cg_up_m,
        cg_toe_m,
        moi_vertical,
        moi_heel_toe,
        moi_forward,
        bulge_radius_m,
        roll_radius_m,
        face_center_height_m,
        face_height_m,
        face_half_width_m,
        face_friction,
        tangential_restitution_up,
        tangential_restitution_toe,
        tangential_restitution_speed_slope,
        arc_radius_m,
        sole_width_m,
        sole_bounce_rad,
    )
}

fn delivery(input: &Dict) -> Delivery {
    read_fields!(
        input,
        Delivery,
        club_speed_mps,
        path_rad,
        attack_rad,
        face_rad,
        dynamic_loft_rad,
        dynamic_lie_rad,
        strike_toe_m,
        sole_height_m,
    )
}

fn ball(input: &Dict) -> BallSpec {
    read_fields!(input, BallSpec, mass_kg, radius_m, inertia_factor)
}

fn lie(input: &Dict) -> Lie {
    read_fields!(
        input,
        Lie {
            surface: Surface::ALL[integer(input, "surface")]
        },
        ball_height_m,
        grass_height_m,
        grass_density,
        sand_firmness,
    )
}

fn ground_sample(input: &Dict) -> GroundSample {
    read_fields!(
        input,
        GroundSample {
            surface: Surface::ALL[integer(input, "surface")],
            normal: vec3(input, "normal")
        },
        height_m,
        firmness,
        moisture,
        green_speed_m,
        grass_height_m,
        grass_density,
    )
}

struct DivotGround {
    depth: Real,
    back: Real,
}

impl GroundSampler for DivotGround {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let inside = x.abs() < 0.03 && z >= -0.05 && z <= self.back;
        GroundSample {
            height_m: if inside { -self.depth } else { 0.0 },
            ..GroundSample::default()
        }
    }
}

struct GroundReads<'a> {
    reads: &'a [Variant],
    index: Cell<usize>,
}

impl GroundSampler for GroundReads<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let i = self.index.get();
        let read = dict(&self.reads[i]);
        self.index.set(i + 1);
        for (axis, value) in [("x", x), ("z", z)] {
            let expected = number(read, axis);
            assert!(
                (value - expected).abs() <= expected.abs().max(1.0) * 8.0 / 8_388_608.0,
                "ground read {i} {axis}: {value} vs {expected}",
            );
        }
        ground_sample(dict(field(read, "sample")))
    }
}

fn with_ground<T>(input: &Dict, run: impl FnOnce(&dyn GroundSampler) -> T) -> T {
    let ground = dict(field(input, "ground"));
    match field(ground, "kind").as_str().unwrap() {
        "plane" => {
            let mut plane = PlaneGround::new(
                ground_sample(dict(field(ground, "base"))),
                vec3(ground, "origin"),
            );
            for patch in array(field(ground, "patches")).iter().map(dict) {
                plane.add_patch(
                    vec2(patch, "min"),
                    vec2(patch, "max"),
                    ground_sample(dict(field(patch, "sample"))),
                );
            }
            run(&plane)
        }
        "course_ground" => {
            let ground = GroundReads {
                reads: array(field(input, "ground_reads")),
                index: Cell::new(0),
            };
            let output = run(&ground);
            assert_eq!(ground.index.get(), ground.reads.len());
            output
        }
        "test_sampler" => {
            assert_eq!(
                field(ground, "script")
                    .as_str()
                    .unwrap()
                    .rsplit("::")
                    .next(),
                Some("DivotGround"),
            );
            let params = dict(field(ground, "parameters"));
            run(&read_fields!(params, DivotGround, depth, back))
        }
        kind => panic!("unknown ground kind {kind}"),
    }
}

pub fn launch_output(launch: Launch) -> Variant {
    record! {
        position: vector(launch.position),
        velocity: vector(launch.velocity),
        spin: vector(launch.spin),
        aim: vector(launch.aim),
        ball_speed_mps: launch.readout.ball_speed_mps,
        launch_angle_rad: launch.readout.launch_angle_rad,
        launch_direction_rad: launch.readout.launch_direction_rad,
        spin_rate_rad_s: launch.readout.spin_rate_rad_s,
        spin_axis_rad: launch.readout.spin_axis_rad,
        club_speed_mps: launch.club_speed_mps,
        club_speed_at_ball_mps: launch.club_speed_at_ball_mps,
        smash_factor: launch.smash_factor,
        club_path_rad: launch.club_path_rad,
        attack_angle_rad: launch.attack_angle_rad,
        face_angle_rad: launch.face_angle_rad,
        dynamic_loft_rad: launch.dynamic_loft_rad,
        face_to_path_rad: launch.face_to_path_rad,
        spin_loft_rad: launch.spin_loft_rad,
        valid: Variant::Bool(launch.valid),
        contact: launch.contact as i64,
        strike_toe_m: launch.strike_toe_m,
        strike_up_m: launch.strike_up_m,
        low_point_ahead_m: launch.low_point_ahead_m,
        low_point_height_m: launch.low_point_height_m,
        turf_entry_behind_m: launch.turf_entry_behind_m,
        turf_contact_start: turf_point(launch.turf_contact_start),
        turf_contact_end: turf_point(launch.turf_contact_end),
        turf_contact_depth_m: launch.turf_contact_depth_m,
        interference: launch.interference,
        ball_temperature_c: launch.ball_temperature_c,
    }
}

fn replay(case: &Dict) -> Result<Variant, String> {
    let call = field(case, "call").as_str().unwrap();
    let input = dict(field(case, "input"));
    let sub = |key| dict(field(input, key));
    let result = match call {
        "club_impact.head_orientation" => {
            let head = impact::head_orientation(
                &club(sub("club")),
                &delivery(sub("d")),
                vec3(input, "f"),
                vec3(input, "s"),
                number(input, "close_rad"),
            );
            record! {
                normal: vector(head.normal),
                face_up: vector(head.face_up),
                toe: vector(head.toe),
                forward: vector(head.forward),
                up: vector(head.up),
            }
        }
        "club_impact.sole_height_for_strike" => impact::sole_height_for_strike(
            &club(sub("club")),
            &ball(sub("ball")),
            &delivery(sub("d")),
            &lie(sub("lie")),
            number(input, "up"),
        )
        .into(),
        "strike_contact.before_ball" => {
            let mut request = ShotRequest::new(
                club(sub("club")),
                BallSpec {
                    radius_m: number(input, "ball_radius"),
                    ..BallSpec::default()
                },
                delivery(sub("d")),
                lie(sub("lie")),
            );
            request.position = vec3(input, "ground_point");
            let pre = with_ground(input, |ground| {
                strike::before_ball(&request, vec3(input, "f"), vec3(input, "s"), ground)
            });
            record! {
                club_speed_mps: pre.club_speed_mps,
                face_close_rad: pre.face_close_rad,
                interference: pre.interference,
                cushion: pre.cushion,
                sole_height_m: pre.sole_height_m,
                low_point_ahead_m: pre.low_point_ahead_m,
                low_point_height_m: pre.low_point_height_m,
                entry_behind_m: pre.entry_behind_m,
                turf_start: turf_point(pre.turf_start),
                turf_end: turf_point(pre.turf_end),
                turf_depth_m: pre.turf_depth_m,
                sand_fraction: pre.sand_fraction,
                sand_blend: pre.sand_blend,
                sand_transfer: pre.sand_transfer,
                sand_bounce_rad: pre.sand_bounce_rad,
                sand_opening: pre.sand_opening,
                direct_sand_access: pre.direct_sand_access,
                sand_spin_transfer: pre.sand_spin_transfer,
                grass_transfer: pre.grass_transfer,
                grass_film: pre.grass_film,
            }
        }
        "club_impact.strike" => {
            let req = sub("req");
            let part = |key| dict(field(req, key));
            let mut request = ShotRequest::new(
                club(part("club")),
                ball(part("ball")),
                delivery(part("delivery")),
                lie(part("lie")),
            );
            request.position = vec3(req, "position");
            request.aim = vec3(req, "aim");
            let temperature = number(req, "ball_temperature_c");
            request.ball_temperature_c = (!temperature.is_nan()).then_some(temperature);
            let air = AirSample {
                temperature_c: number(sub("air"), "temperature_c"),
                ..AirSample::default()
            };
            launch_output(with_ground(input, |ground| {
                impact::strike(&request, &air, ground, &mut NoProbe)
            }))
        }
        _ => return Err(format!("unknown impact call {call}")),
    };
    Ok(result)
}

// In these cases acos amplifies the rounding of a dot product close to 1.
pub const SPIN_LOFT_CASES: [&str; 3] = [
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001343",
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001423",
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001427",
];

pub(in crate::corpus) fn compare(case: &Dict, drift_report: bool) -> Result<Variant, String> {
    let mut actual = replay(case)?;
    let id = field(case, "id").as_str().unwrap();
    if SPIN_LOFT_CASES.contains(&id) {
        let key = "spin_loft_rad";
        let expected = dict(field(case, "output"));
        let Variant::Dict(actual_fields) = &mut actual else {
            return Err(format!("{id}: expected launch output"));
        };
        let angle = number(expected, key);
        let drift = (number(actual_fields, key) - angle).abs();
        let d = EIGHT_SINGLE_PRECISION_STEPS;
        let bound = (d / fm::sin(angle).abs()).max(fm::sqrt(2.0 * d));
        let steps = drift / (d / 8.0 * angle.abs().max(1.0));
        let detail = format!(
            "case {id} {key}: {drift:.3e} rad ({steps:.2} steps), acos bound {bound:.3e} rad"
        );
        if !drift_report && (!drift.is_finite() || drift > bound) {
            return Err(detail);
        }
        println!("{detail}");
        // The shared checker still compares every other field against the physics row.
        if !drift_report {
            *actual_fields.get_mut(key).unwrap() = field(expected, key).clone();
        }
    }
    Ok(actual)
}
