use golf_sim_base::{ClubId, Ef, Surface};

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
fn engine_order_vectors_are_single_precision_only_under_reference() {
    let bytes = if cfg!(feature = "reference") { 4 } else { 8 };
    assert_eq!(size_of::<Ef>(), bytes);
}
