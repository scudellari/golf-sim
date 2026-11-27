use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec2, Vec3,
    digest::object,
    trace::NoProbe,
    variant::{Dict, Variant},
};
use golf_sim_conditions::lies::{AddressLie, BallLie, DivotLie, Sit};
use golf_sim_stroke::{
    address::Address,
    delivery::{self, BuiltDelivery, DeliveryInput},
};
use golf_sim_swing::execution::Execution;

fn field<'a>(record: &'a Dict, name: &str) -> &'a Variant {
    record.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn dict<'a>(record: &'a Dict, name: &str) -> &'a Dict {
    field(record, name).as_dict().unwrap()
}

fn integer(record: &Dict, name: &str) -> i64 {
    let Variant::Int(value) = field(record, name) else {
        panic!("expected integer at {name}");
    };
    value.value()
}

fn number(record: &Dict, name: &str) -> Real {
    match field(record, name) {
        Variant::Int(value) => value.value() as Real,
        Variant::Float(value) => value.value(),
        _ => panic!("expected number at {name}"),
    }
}

fn boolean(record: &Dict, name: &str) -> bool {
    let Variant::Bool(value) = field(record, name) else {
        panic!("expected bool at {name}");
    };
    *value
}

fn surface(record: &Dict, name: &str) -> Surface {
    Surface::ALL[usize::try_from(integer(record, name)).unwrap()]
}

pub fn read_case(case: &Dict) -> (DeliveryInput, Execution, bool) {
    assert_eq!(field(case, "call").as_str(), Some("delivery_builder.build"));

    let args = dict(case, "input");
    let input = dict(args, "input");
    let lie = dict(input, "lie");
    let ball = dict(lie, "ball");
    let Variant::Vector3(spot) = field(ball, "spot") else {
        panic!("expected ball spot");
    };
    let [x, y, z] = spot.values;
    let ball = BallLie {
        spot: Vec3::new(x, y, z),
        surface: surface(ball, "surface"),
        sit: [Sit::Up, Sit::Normal, Sit::Down][integer(ball, "sit") as usize],
        ball_height_m: number(ball, "ball_height_m"),
        grass_height_m: number(ball, "grass_height_m"),
        grass_density: number(ball, "grass_density"),
        sand_firmness: number(ball, "sand_firmness"),
        divot: [
            DivotLie::None,
            DivotLie::Fresh,
            DivotLie::Old,
            DivotLie::SandFilled,
        ][integer(ball, "divot") as usize],
    };
    let input = DeliveryInput {
        club: ClubId::from_name(field(input, "club_id").as_str().unwrap()).unwrap(),
        address: Address::from_dict(dict(input, "address")),
        lie: AddressLie {
            ball,
            slope_along_rad: number(lie, "slope_along_rad"),
            slope_across_rad: number(lie, "slope_across_rad"),
            ball_slope_along_rad: number(lie, "ball_slope_along_rad"),
            lead_foot_surface: surface(lie, "lead_foot_surface"),
            trail_foot_surface: surface(lie, "trail_foot_surface"),
            club_entry_surface: surface(lie, "club_entry_surface"),
        },
        attempt_seed: Seed(integer(input, "attempt_seed")),
    };
    let execution = dict(args, "execution");
    let Variant::Vector2(strike) = field(execution, "strike_m") else {
        panic!("expected strike on the face");
    };
    let [toe, up] = strike.values;
    let execution = Execution {
        power: number(execution, "power"),
        timing_fault: number(execution, "timing_fault"),
        timed_out: boolean(execution, "timed_out"),
        strike_m: boolean(execution, "has_strike").then_some(Vec2::new(toe, up)),
        timing_error_s: 0.0,
        impact_usec: 0,
        backswing_s: 0.0,
        ring_s: 0.0,
    };
    (input, execution, boolean(args, "with_noise"))
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn output(built: &BuiltDelivery) -> Variant {
    let BuiltDelivery {
        club,
        delivery,
        lie,
        fault,
    } = built;
    record! {
        club: record! {
            id: club.id.as_str(),
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
        },
        delivery: record! {
            club_speed_mps: delivery.club_speed_mps,
            path_rad: delivery.path_rad,
            attack_rad: delivery.attack_rad,
            face_rad: delivery.face_rad,
            dynamic_loft_rad: delivery.dynamic_loft_rad,
            dynamic_lie_rad: delivery.dynamic_lie_rad,
            strike_toe_m: delivery.strike_toe_m,
            sole_height_m: delivery.sole_height_m,
        },
        lie: record! {
            surface: lie.surface as i64,
            ball_height_m: lie.ball_height_m,
            grass_height_m: lie.grass_height_m,
            grass_density: lie.grass_density,
            sand_firmness: lie.sand_firmness,
        },
        fault: *fault,
    }
}

pub(in crate::corpus) fn run(case: &Dict) -> Variant {
    let (input, execution, noise) = read_case(case);
    output(&delivery::build(&input, execution, noise, &mut NoProbe))
}
