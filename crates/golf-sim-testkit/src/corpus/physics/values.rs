use golf_sim_base::{
    ClubId, Real, Surface, Vec2, Vec3,
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler},
    tunables::{self as t, ClubEntry, ObstacleProperties, SurfaceProperties},
    values::{
        BallSpec, ClubSpec, Delivery, Launch, LaunchReadout, ObstacleHit, ShotResult, Zone, units,
    },
};

use crate::tolerance::{CaseOutcome, FieldDeparture, FieldDepartureRule};

pub(super) type Version = (u32, u32, u32);

pub(super) fn version(text: &str) -> Option<Version> {
    let (major, rest) = text.split_once('.')?;
    let (minor, patch) = rest.split_once('.')?;
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

pub(super) fn recorded_tunables(header: &Dict) -> Option<&str> {
    super::super::child(header, "oracle")
        .and_then(|oracle| super::super::child(oracle, "versions"))
        .and_then(|versions| versions.get("tunables.VERSION"))
        .and_then(Variant::as_str)
}

pub(in crate::corpus) struct TunableDepartures {
    recorded: Option<String>,
    rules: Vec<FieldDepartureRule>,
}

impl TunableDepartures {
    pub(in crate::corpus) fn new(
        header: &Dict,
        rules: Vec<FieldDepartureRule>,
    ) -> super::super::Result<Self> {
        let recorded = recorded_tunables(header);
        let recorded_version = recorded.and_then(version);
        let current = version(t::VERSION).expect("physics model version");
        let mut applicable = Vec::new();
        for rule in rules {
            let cutoff = version(&rule.version).ok_or_else(|| {
                format!(
                    "invalid physics version {:?} in field departure",
                    rule.version
                )
            })?;
            if recorded_version.is_some_and(|recorded| recorded < cutoff && cutoff <= current) {
                applicable.push(rule);
            }
        }
        Ok(Self {
            recorded: recorded.map(str::to_owned),
            rules: applicable,
        })
    }

    pub(in crate::corpus) fn run(
        &self,
        case: &Dict,
        drift: bool,
    ) -> super::super::Result<CaseOutcome> {
        let mut actual = run_case(case)?;
        let call = field(case, "call").as_str().unwrap();
        let expected = field(case, "output");
        let mut departures = Vec::new();
        match call {
            "tunables.constants" => {
                let mut actual_fields = Vec::new();
                for (key, value) in dict(&actual).entries() {
                    let name = key.as_str().expect("named tunable");
                    let compared = match dict(expected).get(name) {
                        Some(recorded)
                            if golf_sim_base::corpus::first_difference(recorded, value)
                                .is_none() =>
                        {
                            Some(value)
                        }
                        recorded => {
                            let explained = if name == "VERSION" {
                                recorded.and_then(Variant::as_str) == self.recorded.as_deref()
                            } else {
                                recorded.is_none()
                            };
                            if let Some(rule) = self.rule(call, name).filter(|_| explained) {
                                departures.push(self.note(case, name, rule));
                                recorded
                            } else {
                                Some(value)
                            }
                        }
                    };
                    if let Some(compared) = compared {
                        actual_fields.push((key.clone(), compared.clone()));
                    }
                }
                if !drift {
                    actual = Variant::Dict(Dict::new(actual_fields));
                    if let Some(field) = golf_sim_base::corpus::first_difference(expected, &actual)
                    {
                        return Err(format!("tunables.constants differs at {field}").into());
                    }
                }
            }
            "tunables.VERSION"
                if expected.as_str() == self.recorded.as_deref()
                    && expected.as_str() != Some(t::VERSION) =>
            {
                if let Some(rule) = self.rule(call, "$") {
                    departures.push(self.note(case, "$", rule));
                    if !drift {
                        actual = expected.clone();
                    }
                }
            }
            _ => {}
        }
        Ok(CaseOutcome::Compared { actual, departures })
    }

    fn rule(&self, call: &str, field: &str) -> Option<&FieldDepartureRule> {
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.call == call && rule.fields.iter().any(|name| name == field))
    }

    fn note(&self, case: &Dict, field_name: &str, rule: &FieldDepartureRule) -> FieldDeparture {
        FieldDeparture {
            id: field(case, "id").as_str().unwrap().into(),
            field: field_name.into(),
            change: rule.change.clone(),
        }
    }
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected dictionary")
}

fn array(value: &Variant) -> &[Variant] {
    match value {
        Variant::Array(array) => &array.items,
        _ => panic!("expected array"),
    }
}

fn field<'a>(record: &'a Dict, key: &str) -> &'a Variant {
    record
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

fn number(record: &Dict, key: &str) -> Real {
    match field(record, key) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected number at {key}"),
    }
}

fn integer(value: &Variant) -> i64 {
    match value {
        Variant::Int(value) => value.value(),
        _ => panic!("expected integer"),
    }
}

fn vec2(value: &Variant) -> Vec2 {
    match value {
        Variant::Vector2(value) => Vec2::new(value.values[0], value.values[1]),
        _ => panic!("expected Vector2"),
    }
}

fn vec3(value: &Variant) -> Vec3 {
    match value {
        Variant::Vector3(value) => {
            let [x, y, z] = value.values;
            Vec3::new(x, y, z)
        }
        _ => panic!("expected Vector3"),
    }
}

fn vector(value: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [value.x, value.y, value.z],
        width: FloatWidth::Double,
    })
}

fn surface(code: i64) -> Surface {
    Surface::ALL[usize::try_from(code).unwrap()]
}

fn zone(code: i64) -> Zone {
    match code {
        0 => Zone::InPlay,
        1 => Zone::PenaltyArea,
        2 => Zone::OutOfBounds,
        _ => panic!("unknown zone {code}"),
    }
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        Variant::Dict(Dict::new(vec![$((stringify!($key).into(), ($value).into())),+]))
    };
}

fn ball_input(value: &Variant) -> BallSpec {
    let value = dict(value);
    BallSpec {
        mass_kg: number(value, "mass_kg"),
        radius_m: number(value, "radius_m"),
        inertia_factor: number(value, "inertia_factor"),
    }
}

fn ball_output(ball: BallSpec) -> Variant {
    record! {
        mass_kg: ball.mass_kg,
        radius_m: ball.radius_m,
        inertia_factor: ball.inertia_factor,
    }
}

fn club_output(club: ClubSpec) -> Variant {
    record! {
        id: Variant::String(club.id),
        category: club.category as i64,
        loft_rad: club.loft_rad,
        length_m: club.length_m,
        head_mass_kg: club.head_mass_kg,
        cor: club.cor,
        cor_speed_slope_delta: club.cor_speed_slope_delta,
        cor_falloff_toe: club.cor_falloff_toe,
        cor_falloff_up: club.cor_falloff_up,
        cg_depth_m: club.cg_depth_m,
        cg_up_m: club.cg_up_m,
        cg_toe_m: club.cg_toe_m,
        moi_vertical: club.moi_vertical,
        moi_heel_toe: club.moi_heel_toe,
        moi_forward: club.moi_forward,
        bulge_radius_m: club.bulge_radius_m,
        roll_radius_m: club.roll_radius_m,
        face_center_height_m: club.face_center_height_m,
        face_height_m: club.face_height_m,
        face_half_width_m: club.face_half_width_m,
        face_friction: club.face_friction,
        tangential_restitution_up: club.tangential_restitution_up,
        tangential_restitution_toe: club.tangential_restitution_toe,
        tangential_restitution_speed_slope: club.tangential_restitution_speed_slope,
        arc_radius_m: club.arc_radius_m,
        sole_width_m: club.sole_width_m,
        sole_bounce_rad: club.sole_bounce_rad,
    }
}

fn ground_input(value: &Variant) -> GroundSample {
    let d = dict(value);
    GroundSample {
        surface: surface(integer(field(d, "surface"))),
        height_m: number(d, "height_m"),
        normal: vec3(field(d, "normal")),
        firmness: number(d, "firmness"),
        moisture: number(d, "moisture"),
        green_speed_m: number(d, "green_speed_m"),
        grass_height_m: number(d, "grass_height_m"),
        grass_density: number(d, "grass_density"),
    }
}

fn ground_output(sample: GroundSample) -> Variant {
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

fn set_zones(plane: &mut PlaneGround, value: &Dict) {
    plane.set_outside_zone(zone(integer(field(value, "outside"))));
    let Variant::PackedInt32Array(kinds) = field(value, "kinds") else {
        panic!("expected zone kinds");
    };
    let polygons = array(field(value, "polygons"));
    assert_eq!(kinds.len(), polygons.len());
    for (&kind, polygon) in kinds.iter().zip(polygons) {
        let Variant::PackedVector2Array(points) = polygon else {
            panic!("expected zone polygon");
        };
        let &[min, bottom_right, max, top_left] = points.values.as_slice() else {
            panic!("expected rectangular zone");
        };
        assert_eq!(bottom_right, [max[0], min[1]]);
        assert_eq!(top_left, [min[0], max[1]]);
        plane.add_zone(
            Vec2::new(min[0], min[1]),
            Vec2::new(max[0], max[1]),
            zone(kind.into()),
        );
    }
}

fn plane_input(value: &Variant) -> PlaneGround {
    let d = dict(value);
    assert_eq!(field(d, "kind").as_str(), Some("plane"));
    let mut plane = PlaneGround::new(ground_input(field(d, "base")), vec3(field(d, "origin")));
    for patch in array(field(d, "patches")).iter().map(dict) {
        plane.add_patch(
            vec2(field(patch, "min")),
            vec2(field(patch, "max")),
            ground_input(field(patch, "sample")),
        );
    }
    for trunk in array(field(d, "trunks")).iter().map(dict) {
        plane.add_trunk(
            vec2(field(trunk, "c")),
            number(trunk, "r"),
            number(trunk, "h"),
        );
    }
    for canopy in array(field(d, "canopies")).iter().map(dict) {
        plane.add_canopy(
            vec3(field(canopy, "c")),
            number(canopy, "r"),
            number(canopy, "d"),
        );
    }
    if let Some(zones) = field(d, "zones").as_dict() {
        set_zones(&mut plane, zones);
    }
    plane
}

fn hit_output(hit: ObstacleHit) -> Variant {
    record! {
        kind: hit.kind as i64,
        point: vector(hit.point),
        normal: vector(hit.normal),
        fraction: hit.fraction,
    }
}

fn air_input(value: &Variant) -> ConstantAir {
    let d = dict(value);
    assert_eq!(field(d, "kind").as_str(), Some("constant_air"));
    let air = dict(field(d, "air"));
    let mut conditions = ConstantAir::new(AirSample {
        temperature_c: number(air, "temperature_c"),
        pressure_pa: number(air, "pressure_pa"),
        relative_humidity: number(air, "relative_humidity"),
        mean_wind_mps: vec3(field(air, "mean_wind_mps")),
        gust_mps: vec3(field(air, "gust_mps")),
        wind_reference_height_m: number(air, "wind_reference_height_m"),
        roughness_length_m: number(air, "roughness_length_m"),
    });
    for gust in array(field(d, "gusts")).iter().map(dict) {
        conditions.add_gust(
            number(gust, "start"),
            number(gust, "end"),
            vec3(field(gust, "gust")),
        );
    }
    conditions
}

fn air_output(air: AirSample) -> Variant {
    record! {
        temperature_c: air.temperature_c,
        pressure_pa: air.pressure_pa,
        relative_humidity: air.relative_humidity,
        mean_wind_mps: vector(air.mean_wind_mps),
        gust_mps: vector(air.gust_mps),
        wind_reference_height_m: air.wind_reference_height_m,
        roughness_length_m: air.roughness_length_m,
    }
}

fn shot_samples(value: &Variant) -> ShotResult {
    let d = dict(value);
    let Variant::PackedFloat64Array(times) = field(d, "times") else {
        panic!("expected playback times");
    };
    let Variant::PackedVector3Array(positions) = field(d, "positions") else {
        panic!("expected playback positions");
    };
    ShotResult {
        times: times.clone(),
        positions: positions
            .values
            .iter()
            .map(|&[x, y, z]| Vec3::new(x, y, z))
            .collect(),
        rest_position: vec3(field(d, "rest_position")),
        ..ShotResult::default()
    }
}

fn launch_output(launch: Launch) -> Variant {
    let readout = launch.readout;
    let absent = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);
    record! {
        position: vector(launch.position),
        velocity: vector(launch.velocity),
        spin: vector(launch.spin),
        aim: vector(launch.aim),
        ball_speed_mps: readout.ball_speed_mps,
        launch_angle_rad: readout.launch_angle_rad,
        launch_direction_rad: readout.launch_direction_rad,
        spin_rate_rad_s: readout.spin_rate_rad_s,
        spin_axis_rad: readout.spin_axis_rad,
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
        turf_contact_start: vector(launch.turf_contact_start.unwrap_or(absent)),
        turf_contact_end: vector(launch.turf_contact_end.unwrap_or(absent)),
        turf_contact_depth_m: launch.turf_contact_depth_m,
        interference: launch.interference,
        ball_temperature_c: launch.ball_temperature_c,
    }
}

fn surface_output(sample: SurfaceProperties) -> Variant {
    let mut fields = vec![
        ("cor".into(), sample.cor.into()),
        ("crater".into(), sample.crater.into()),
    ];
    if sample.hard {
        fields.push(("hard".into(), Variant::Bool(sample.hard)));
    }
    fields.extend([
        ("friction".into(), sample.friction.into()),
        ("stimp_m".into(), sample.stimp_m.into()),
        (
            "uses_green_speed".into(),
            Variant::Bool(sample.uses_green_speed),
        ),
    ]);
    if sample.landing_loss != 0.0 {
        fields.push(("landing_loss".into(), sample.landing_loss.into()));
    }
    fields.extend([
        ("grass_drag".into(), sample.grass_drag.into()),
        ("skid_friction".into(), sample.skid_friction.into()),
        ("cut_energy".into(), sample.cut_energy.into()),
    ]);
    Variant::Dict(Dict::new(fields))
}

fn obstacle_output(obstacle: ObstacleProperties) -> Variant {
    record! { cor: obstacle.cor, friction: obstacle.friction }
}

fn club_entry_output(entry: ClubEntry) -> Variant {
    let h = entry.head;
    let mut fields = vec![
        ("category".into(), (entry.category as i64).into()),
        ("loft_deg".into(), entry.loft_deg.into()),
        ("length_in".into(), entry.length_in.into()),
        ("mass_g".into(), entry.mass_g.into()),
        ("cg_depth".into(), h.cg_depth.into()),
        ("cg_up".into(), h.cg_up.into()),
        ("moi_v".into(), h.moi_v.into()),
        ("moi_ht".into(), h.moi_ht.into()),
        ("moi_f".into(), h.moi_f.into()),
        ("bulge_in".into(), h.bulge_in.into()),
        ("roll_in".into(), h.roll_in.into()),
        ("face_h".into(), h.face_h.into()),
        ("face_c".into(), h.face_c.into()),
        ("face_w".into(), h.face_w.into()),
        ("mu".into(), h.mu.into()),
    ];
    if let Some(toe_fit) = h.e_t_toe {
        fields.push(("e_t_toe".into(), toe_fit.into()));
    }
    fields.extend([
        ("arc".into(), h.arc.into()),
        ("sole_w".into(), h.sole_w.into()),
        ("falloff_toe".into(), h.falloff_toe.into()),
        ("falloff_up".into(), h.falloff_up.into()),
    ]);
    Variant::Dict(Dict::new(fields))
}

fn real_array(values: impl IntoIterator<Item = Real>) -> Variant {
    Variant::Array(Array::new(values.into_iter().map(Variant::from).collect()))
}

fn keyed(key_values: impl IntoIterator<Item = (Variant, Variant)>) -> Variant {
    Variant::Dict(Dict::new(key_values.into_iter().collect()))
}

fn indexed<T: Copy>(values: &[T], output: impl Fn(T) -> Variant) -> Variant {
    keyed(
        values
            .iter()
            .copied()
            .enumerate()
            .map(|(index, value)| ((index as i64).into(), output(value))),
    )
}

fn tunable_snapshot() -> Variant {
    record! {
        VERSION: t::VERSION,
        GRAVITY: t::GRAVITY,
        KELVIN: t::KELVIN,
        MIN_WIND_REFERENCE_RATIO: t::MIN_WIND_REFERENCE_RATIO,
        R_DRY: t::R_DRY,
        R_VAPOR: t::R_VAPOR,
        BUCK_A_PA: t::BUCK_A_PA,
        BUCK_B: t::BUCK_B,
        BUCK_C: t::BUCK_C,
        BUCK_D: t::BUCK_D,
        SUTHERLAND_MU0: t::SUTHERLAND_MU0,
        SUTHERLAND_T0: t::SUTHERLAND_T0,
        SUTHERLAND_S: t::SUTHERLAND_S,
        ISA_P0: t::ISA_P0,
        STANDARD_TEMPERATURE_C: t::STANDARD_TEMPERATURE_C,
        STANDARD_HUMIDITY: t::STANDARD_HUMIDITY,
        ISA_T0: t::ISA_T0,
        ISA_LAPSE: t::ISA_LAPSE,
        ISA_EXPONENT: t::ISA_EXPONENT,
        FLIGHT_DT: t::FLIGHT_DT,
        GROUND_DT: t::GROUND_DT,
        PUTTING_GROUND_DT: t::PUTTING_GROUND_DT,
        SAMPLE_INTERVAL_S: t::SAMPLE_INTERVAL_S,
        MAX_SHOT_TIME_S: t::MAX_SHOT_TIME_S,
        CONTACT_SLOP_M: t::CONTACT_SLOP_M,
        MIN_HOP_HEIGHT_M: t::MIN_HOP_HEIGHT_M,
        SUPPORTED_LIE_HEIGHT_M: t::SUPPORTED_LIE_HEIGHT_M,
        SUPPORT_BLEND_HEIGHT_M: t::SUPPORT_BLEND_HEIGHT_M,
        TERRAIN_SWEEP_M: t::TERRAIN_SWEEP_M,
        TERRAIN_REFINE_STEPS: t::TERRAIN_REFINE_STEPS as i64,
        SOLID_SWEEP_ITERATIONS: t::SOLID_SWEEP_ITERATIONS as i64,
        MAX_SIMULTANEOUS_CONTACTS: t::MAX_SIMULTANEOUS_CONTACTS as i64,
        CD_BASE: t::CD_BASE,
        CD_SPIN: t::CD_SPIN,
        CD_SPIN2: t::CD_SPIN2,
        CD_CRISIS_RISE: t::CD_CRISIS_RISE,
        CL_SCALE: t::CL_SCALE,
        CL_EXPONENT: t::CL_EXPONENT,
        CD_SPIN_PEAK: t::CD_SPIN_PEAK,
        CD_CRISIS_RE: t::CD_CRISIS_RE,
        CD_CRISIS_WIDTH: t::CD_CRISIS_WIDTH,
        SPIN_DECAY_LAMBDA: t::SPIN_DECAY_LAMBDA,
        IMPACT_FITS: keyed(t::IMPACT_FITS.map(|(club, fit)| (club.name().into(), real_array(fit)))),
        DRIVER_MOI_HEEL_TOE: t::DRIVER_MOI_HEEL_TOE,
        DRIVER_TANGENTIAL_SPEED_SLOPE: t::DRIVER_TANGENTIAL_SPEED_SLOPE,
        IRON_COR_SPEED_SLOPE_DELTA: t::IRON_COR_SPEED_SLOPE_DELTA,
        COR_REFERENCE_SPEED: t::COR_REFERENCE_SPEED,
        COR_SPEED_SLOPE: t::COR_SPEED_SLOPE,
        COR_LOW_SPEED_MAX: t::COR_LOW_SPEED_MAX,
        COR_PER_DEGREE_C: t::COR_PER_DEGREE_C,
        COR_TEMPERATURE_REFERENCE_C: t::COR_TEMPERATURE_REFERENCE_C,
        EDGE_COR_FACTOR: t::EDGE_COR_FACTOR,
        EDGE_COR_BLEND_M: t::EDGE_COR_BLEND_M,
        DRIVER_SMASH_LIMIT: t::DRIVER_SMASH_LIMIT,
        TANGENTIAL_SPEED_RANGE_MPS: t::TANGENTIAL_SPEED_RANGE_MPS,
        INTERFERENCE_COR_LOSS: t::INTERFERENCE_COR_LOSS,
        INTERFERENCE_FRICTION_LOSS: t::INTERFERENCE_FRICTION_LOSS,
        INTERFERENCE_TANGENTIAL_LOSS: t::INTERFERENCE_TANGENTIAL_LOSS,
        ARC_LOOKBACK_M: t::ARC_LOOKBACK_M,
        ARC_SAMPLES: t::ARC_SAMPLES as i64,
        CUT_REFERENCE_SPEED: t::CUT_REFERENCE_SPEED,
        CUT_STATIC_FRACTION: t::CUT_STATIC_FRACTION,
        SOLE_BOUNCE_DEPTH_RELIEF: t::SOLE_BOUNCE_DEPTH_RELIEF,
        HARD_SOLE_FRICTION: t::HARD_SOLE_FRICTION,
        HARD_SOLE_IMPULSE_DEPTH_M: t::HARD_SOLE_IMPULSE_DEPTH_M,
        TURF_BLOCK_LOOKBACK_M: t::TURF_BLOCK_LOOKBACK_M,
        TURF_FULL_BLOCK_DEPTH_M: t::TURF_FULL_BLOCK_DEPTH_M,
        SAND_FULL_BLOCK_DEPTH_M: t::SAND_FULL_BLOCK_DEPTH_M,
        GRASS_CUT_ENERGY: t::GRASS_CUT_ENERGY,
        GRASS_ABOVE_HEAD_SHARE: t::GRASS_ABOVE_HEAD_SHARE,
        GRASS_FACE_CLOSE_DEG_PER_J: t::GRASS_FACE_CLOSE_DEG_PER_J,
        GRASS_FACE_CLOSE_MAX_DEG: t::GRASS_FACE_CLOSE_MAX_DEG,
        GRASS_CLOSE_BY_CATEGORY: real_array(t::GRASS_CLOSE_BY_CATEGORY),
        GRASS_FULL_BLOCK_M: t::GRASS_FULL_BLOCK_M,
        GRASS_FILM_START_M: t::GRASS_FILM_START_M,
        GRASS_FILM_FULL_M: t::GRASS_FILM_FULL_M,
        GRASS_FILM_FRICTION_DECAY: t::GRASS_FILM_FRICTION_DECAY,
        GRASS_COVER_BALL_RADII: t::GRASS_COVER_BALL_RADII,
        GRASS_ESCAPE_START_DEG: t::GRASS_ESCAPE_START_DEG,
        GRASS_ESCAPE_FULL_DEG: t::GRASS_ESCAPE_FULL_DEG,
        GRASS_COVER_LOSS_SHALLOW: t::GRASS_COVER_LOSS_SHALLOW,
        GRASS_COVER_LOSS_STEEP: t::GRASS_COVER_LOSS_STEEP,
        SAND_TRANSFER_LENGTH_M: t::SAND_TRANSFER_LENGTH_M,
        SAND_BLEND_AREA_M2: t::SAND_BLEND_AREA_M2,
        SAND_LIFT_START_DEG: t::SAND_LIFT_START_DEG,
        SAND_LIFT_FULL_DEG: t::SAND_LIFT_FULL_DEG,
        SAND_OPEN_BOUNCE: t::SAND_OPEN_BOUNCE,
        SAND_BOUNCE_GAIN: t::SAND_BOUNCE_GAIN,
        GRASS_PATH_DEFAULT_DENSITY: t::GRASS_PATH_DEFAULT_DENSITY,
        GRASS_PATH_DEFAULT_HEIGHT: keyed(t::GRASS_PATH_DEFAULT_HEIGHT.map(|(kind, height)| ((kind as i64).into(), height.into()))),
        WEDGE_SOLE_BOUNCE_DEG: t::WEDGE_SOLE_BOUNCE_DEG,
        SAND_CUSHION_DECAY_M: t::SAND_CUSHION_DECAY_M,
        SAND_DIRECT_ACCESS_SOFT_M: t::SAND_DIRECT_ACCESS_SOFT_M,
        SAND_DIRECT_ACCESS_FIRM_M: t::SAND_DIRECT_ACCESS_FIRM_M,
        SAND_BURIED_BLEND_M: t::SAND_BURIED_BLEND_M,
        SAND_DIG_CLEARANCE_M: t::SAND_DIG_CLEARANCE_M,
        SAND_EXTRACTION_DECAY_M: t::SAND_EXTRACTION_DECAY_M,
        SAND_BURIED_SPIN_KEEP: t::SAND_BURIED_SPIN_KEEP,
        SAND_FACE_WEIGHT: t::SAND_FACE_WEIGHT,
        SAND_BOUNCE_LAUNCH_GAIN: t::SAND_BOUNCE_LAUNCH_GAIN,
        SAND_OPEN_SPEED_LOSS: t::SAND_OPEN_SPEED_LOSS,
        SAND_SPIN_KEEP: t::SAND_SPIN_KEEP,
        SAND_OPEN_SPIN_LOSS: t::SAND_OPEN_SPIN_LOSS,
        SAND_CUT_SOFT_SCALE: t::SAND_CUT_SOFT_SCALE,
        SAND_CUT_FIRM_SCALE: t::SAND_CUT_FIRM_SCALE,
        SAND_CUT_LOW_LOFT_SCALE: t::SAND_CUT_LOW_LOFT_SCALE,
        SAND_TRANSFER_SOFT_SCALE: t::SAND_TRANSFER_SOFT_SCALE,
        SAND_TRANSFER_FIRM_SCALE: t::SAND_TRANSFER_FIRM_SCALE,
        CRATER_ONSET_MPS: t::CRATER_ONSET_MPS,
        CRATER_FULL_LOAD_MPS: t::CRATER_FULL_LOAD_MPS,
        BOUNCE_CRATER_DEG: t::BOUNCE_CRATER_DEG,
        BOUNCE_CRATER_SPEED: t::BOUNCE_CRATER_SPEED,
        BOUNCE_CRATER_INCIDENCE_DEG: t::BOUNCE_CRATER_INCIDENCE_DEG,
        BOUNCE_COR_A: t::BOUNCE_COR_A,
        BOUNCE_COR_B: t::BOUNCE_COR_B,
        BOUNCE_COR_C: t::BOUNCE_COR_C,
        BOUNCE_COR_KNEE_SPEED: t::BOUNCE_COR_KNEE_SPEED,
        BOUNCE_COR_HIGH_SPEED: t::BOUNCE_COR_HIGH_SPEED,
        BOUNCE_CRATER_SCALE: t::BOUNCE_CRATER_SCALE,
        BOUNCE_CRATER_MAX_DEG: t::BOUNCE_CRATER_MAX_DEG,
        BOUNCE_TURF_FRICTION: t::BOUNCE_TURF_FRICTION,
        BOUNCE_TANGENTIAL_COR: t::BOUNCE_TANGENTIAL_COR,
        BOUNCE_GREEN_MAX_DEG: t::BOUNCE_GREEN_MAX_DEG,
        BOUNCE_GREEN_COR_FLOOR: t::BOUNCE_GREEN_COR_FLOOR,
        BOUNCE_GREEN_TANGENTIAL_COR: t::BOUNCE_GREEN_TANGENTIAL_COR,
        BOUNCE_GREEN_CRATER_SCALE: t::BOUNCE_GREEN_CRATER_SCALE,
        BOUNCE_GREEN_CAP_SOFTNESS: t::BOUNCE_GREEN_CAP_SOFTNESS,
        GREEN_IRON_TILT_DEG: t::GREEN_IRON_TILT_DEG,
        GREEN_IRON_TILT_ANGLE: t::GREEN_IRON_TILT_ANGLE,
        GREEN_IRON_TILT_SPEED: t::GREEN_IRON_TILT_SPEED,
        GREEN_IRON_TILT_FIRMNESS: t::GREEN_IRON_TILT_FIRMNESS,
        GREEN_IRON_COR: t::GREEN_IRON_COR,
        GREEN_IRON_COR_ANGLE: t::GREEN_IRON_COR_ANGLE,
        GREEN_IRON_COR_ANGLE_SQUARED: t::GREEN_IRON_COR_ANGLE_SQUARED,
        GREEN_IRON_COR_FIRMNESS: t::GREEN_IRON_COR_FIRMNESS,
        GREEN_IRON_FRICTION: t::GREEN_IRON_FRICTION,
        GREEN_IRON_FRICTION_ANGLE: t::GREEN_IRON_FRICTION_ANGLE,
        GREEN_IRON_FRICTION_FIRMNESS: t::GREEN_IRON_FRICTION_FIRMNESS,
        GREEN_IRON_FRICTION_SPEED_SQUARED: t::GREEN_IRON_FRICTION_SPEED_SQUARED,
        BOUNCE_CRATER_SOFT_SCALE: t::BOUNCE_CRATER_SOFT_SCALE,
        BOUNCE_CRATER_FIRM_SCALE: t::BOUNCE_CRATER_FIRM_SCALE,
        BOUNCE_COR_SOFT_SCALE: t::BOUNCE_COR_SOFT_SCALE,
        BOUNCE_COR_FIRM_SCALE: t::BOUNCE_COR_FIRM_SCALE,
        BOUNCE_COR_WET_LOSS: t::BOUNCE_COR_WET_LOSS,
        BOUNCE_SOFT_FIRMNESS_LOSS: t::BOUNCE_SOFT_FIRMNESS_LOSS,
        BOUNCE_SOFT_MOISTURE_LOSS: t::BOUNCE_SOFT_MOISTURE_LOSS,
        STIMP_EXIT_SPEED: t::STIMP_EXIT_SPEED,
        ROLL_STOP_SPEED: t::ROLL_STOP_SPEED,
        ROLL_MOISTURE_SLOWDOWN: t::ROLL_MOISTURE_SLOWDOWN,
        ROLL_FIRMNESS_GAIN: t::ROLL_FIRMNESS_GAIN,
        MOISTURE_REFERENCE: t::MOISTURE_REFERENCE,
        FAIRWAY_STIMP_M: t::FAIRWAY_STIMP_M,
        SURFACES: indexed(&t::SURFACES, surface_output),
        PITCH_DEPTH_PER_MPS: t::PITCH_DEPTH_PER_MPS,
        PITCH_MOISTURE_GAIN: t::PITCH_MOISTURE_GAIN,
        CUP_RADIUS_M: t::CUP_RADIUS_M,
        CUP_DEPTH_M: t::CUP_DEPTH_M,
        CUP_CAPTURE_SPEED: t::CUP_CAPTURE_SPEED,
        CUP_CAPTURE_EXPONENT: t::CUP_CAPTURE_EXPONENT,
        CUP_CAPTURE_MAX_SLOPE_RAD: t::CUP_CAPTURE_MAX_SLOPE_RAD,
        CUP_LIP_MAX_DEFLECT_RAD: t::CUP_LIP_MAX_DEFLECT_RAD,
        CUP_LIP_SPEED_KEEP: t::CUP_LIP_SPEED_KEEP,
        CANOPY_DRAG_PER_M: t::CANOPY_DRAG_PER_M,
        CANOPY_SPIN_LOSS_PER_S: t::CANOPY_SPIN_LOSS_PER_S,
        OBSTACLES: indexed(&t::OBSTACLES, obstacle_output),
        FLAGSTICK_RADIUS_M: t::FLAGSTICK_RADIUS_M,
        FLAGSTICK_HEIGHT_M: t::FLAGSTICK_HEIGHT_M,
        FLAGSTICK_RESTITUTION: t::FLAGSTICK_RESTITUTION,
        FLAGSTICK_STIFF_RESTITUTION: t::FLAGSTICK_STIFF_RESTITUTION,
        FLAGSTICK_TANGENTIAL_FRICTION: t::FLAGSTICK_TANGENTIAL_FRICTION,
        BALL: ball_output(t::BALL),
        _IN: t::_IN,
        _G_CM2: t::_G_CM2,
        CLUBS: keyed(t::CLUBS.map(|(id, entry)| (id.name().into(), club_entry_output(entry)))),
        PUTTER_COR: t::PUTTER_COR,
    }
}

fn run_case(case: &Dict) -> Result<Variant, String> {
    let call = field(case, "call").as_str().unwrap();
    let input = dict(field(case, "input"));
    let instance = || field(input, "self");
    let real = |key| number(input, key);
    let point = |key| vec3(field(input, key));
    let result = match call {
        "tunables.constants" => tunable_snapshot(),
        "tunables.VERSION" => t::VERSION.into(),
        "tunables.standard_ball" => ball_output(t::standard_ball()),
        "tunables.standard_club_ids" => Variant::PackedStringArray(
            t::standard_club_ids()
                .iter()
                .map(|id| id.name().to_owned())
                .collect(),
        ),
        "tunables.standard_club" => {
            let id = ClubId::from_name(field(input, "id").as_str().unwrap()).unwrap();
            club_output(t::standard_club(id))
        }
        "tunables.surface" => surface_output(*t::surface(surface(integer(field(input, "kind"))))),
        "sim_ball_spec.area" => ball_input(instance()).area().into(),
        "sim_ball_spec.moment_of_inertia" => ball_input(instance()).moment_of_inertia().into(),
        "sim_plane_ground.is_planar" => Variant::Bool(plane_input(instance()).is_planar()),
        "sim_plane_ground.is_uniform_flat" => {
            Variant::Bool(plane_input(instance()).is_uniform_flat())
        }
        "sim_plane_ground.height_at" => plane_input(instance())
            .height_at(real("x"), real("z"))
            .into(),
        "sim_plane_ground.obstacle_hit" => plane_input(instance())
            .obstacle_hit(point("from"), point("to"), real("radius"))
            .map_or(Variant::Nil, hit_output),
        "sim_plane_ground.canopy_density" => plane_input(instance())
            .canopy_density(point("position"))
            .into(),
        "sim_plane_ground.sample_ground" => {
            let position = point("position");
            ground_output(plane_input(instance()).sample_ground(position.x, position.z))
        }
        "sim_zone_map.zone_at" => {
            let mut plane = PlaneGround::default();
            set_zones(&mut plane, dict(instance()));
            (plane.zone_at(point("position")) as i64).into()
        }
        "sim_constant_conditions.sample_air" => {
            air_output(air_input(instance()).sample_air(real("time_s"), point("position")))
        }
        "sim_shot_result.position_at" => vector(shot_samples(instance()).position_at(real("t"))),
        "sim_units.mph" => units::mph(real("value")).into(),
        "sim_units.to_mph" => units::to_mph(real("mps")).into(),
        "sim_units.yards" => units::yards(real("value")).into(),
        "sim_units.to_yards" => units::to_yards(real("meters")).into(),
        "sim_units.feet" => units::feet(real("value")).into(),
        "sim_units.to_feet" => units::to_feet(real("meters")).into(),
        "sim_units.inches" => units::inches(real("value")).into(),
        "sim_units.rpm" => units::rpm(real("value")).into(),
        "sim_units.to_rpm" => units::to_rpm(real("rad_per_s")).into(),
        "sim_units.fahrenheit_to_celsius" => units::fahrenheit_to_celsius(real("value")).into(),
        "sim_delivery.path_on_plane" => Delivery::path_on_plane(
            real("swing_direction_rad"),
            real("plane_rad"),
            real("attack_rad"),
        )
        .into(),
        "sim_launch.from_readout" => launch_output(Launch::from_readout(
            &ball_input(field(input, "ball")),
            point("ground_point"),
            point("aim_dir"),
            LaunchReadout {
                ball_speed_mps: real("ball_speed_mps"),
                launch_angle_rad: real("launch_angle_rad"),
                launch_direction_rad: real("launch_direction_rad"),
                spin_rate_rad_s: real("spin_rate_rad_s"),
                spin_axis_rad: real("spin_axis_rad"),
            },
        )),
        _ => return Err(format!("unknown physics values call {call:?}")),
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golf_sim_base::digest::object;

    #[test]
    fn historical_field_departures_keep_other_tunables_exact() {
        let rules = || {
            vec![FieldDepartureRule {
                call: "tunables.constants".into(),
                fields: vec!["VERSION".into(), "FLAGSTICK_RESTITUTION".into()],
                version: "0.9.0".into(),
                change: "flagstick rim response".into(),
            }]
        };
        let header = |version: &str| {
            object([(
                "oracle",
                object([("versions", object([("tunables.VERSION", version.into())]))]),
            )])
        };
        let historical = header("0.8.0");
        let past = TunableDepartures::new(dict(&historical), rules()).unwrap();
        let snapshot = tunable_snapshot();
        let mut recorded: Vec<_> = dict(&snapshot)
            .entries()
            .iter()
            .filter(|(key, _)| key.as_str() != Some("FLAGSTICK_RESTITUTION"))
            .cloned()
            .collect();
        *recorded
            .iter_mut()
            .find(|(key, _)| key.as_str() == Some("VERSION"))
            .map(|(_, value)| value)
            .unwrap() = "0.8.0".into();
        let case = |entries| {
            object([
                ("id", "synthetic".into()),
                ("call", "tunables.constants".into()),
                ("input", object([])),
                ("output", Variant::Dict(Dict::new(entries))),
            ])
        };
        let mut compatible = case(recorded.clone());
        let CaseOutcome::Compared { departures, .. } = past.run(dict(&compatible), false).unwrap()
        else {
            panic!("expected an older snapshot with field departures");
        };
        assert_eq!(departures.len(), 2);

        let latest = header(t::VERSION);
        let current = TunableDepartures::new(dict(&latest), rules()).unwrap();
        assert!(current.run(dict(&compatible), false).is_err());

        *recorded
            .iter_mut()
            .find(|(key, _)| key.as_str() == Some("GRAVITY"))
            .map(|(_, value)| value)
            .unwrap() = 50.0.into();
        compatible = case(recorded);
        assert!(
            matches!(past.run(dict(&compatible), false), Err(error) if error.to_string().contains("GRAVITY"))
        );
    }
}
