use golf_sim_base::{Basis, ClubId, Real, Surface, Vec2, Vec3, Vec4};

#[test]
fn club_ids_are_the_standard_bag_in_order() {
    let names: Vec<_> = ClubId::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(
        names,
        [
            "driver", "3wood", "hybrid", "4iron", "5iron", "6iron", "7iron", "8iron", "9iron",
            "pw", "gw", "sw", "lw", "putter"
        ]
    );
    for (place, club) in ClubId::ALL.iter().enumerate() {
        assert_eq!(*club as usize, place);
        assert_eq!(ClubId::from_name(club.name()), Some(*club));
    }
    assert_eq!(ClubId::from_name("Driver"), None);
}

#[test]
fn surfaces_keep_the_reference_enum_values_and_bundle_names() {
    assert_eq!(Surface::from_name("TEE").map(|s| s as u8), Some(0));
    assert_eq!(Surface::from_name("GREEN").map(|s| s as u8), Some(6));
    assert_eq!(Surface::from_name("WATER").map(|s| s as u8), Some(10));
    assert_eq!(Surface::ALL.len(), 11);
    for surface in Surface::ALL {
        assert_eq!(Surface::from_name(surface.name()), Some(*surface));
    }
    assert_eq!(Surface::from_name("water"), None);
}

#[test]
fn engine_order_vectors_and_bases_preserve_double_precision() {
    let value = 1.0 + f64::EPSILON;
    let v2 = Vec2 { x: value, y: value };
    let v3 = Vec3 {
        x: value,
        y: value,
        z: value,
    };
    let v4 = Vec4 {
        x: value,
        y: value,
        z: value,
        w: value,
    };
    let basis = Basis { rows: [v3; 3] };
    let components: [Real; 9] = [v2.x, v2.y, v3.x, v3.y, v3.z, v4.x, v4.y, v4.z, v4.w];
    assert_eq!(components, [value; 9]);
    assert_eq!(basis.rows, [v3; 3]);
}
