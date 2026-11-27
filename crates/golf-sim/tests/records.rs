use std::path::{Path, PathBuf};

use golf_sim::{
    records::hole::{Entry, HoleRecord, Refusal, StrokeEntry, SwingPress, decode_envelope},
    setup::{HoleSetup, Mode, PlaySetting, PlaySettings, Profile, StrikeVariant},
};
use golf_sim_base::{
    ClubId, Error, Seed, Vec2,
    corpus::{self, first_difference},
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::course::Course;
use golf_sim_stroke::{address::Address, bag::Group};
use golf_sim_swing::hands::{Profile as Hands, Swing};
use golf_sim_testkit::reference_records::{ReferenceEntry, ReferenceHoleRecord, decode_hole};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn fixture() -> Vec<Variant> {
    let fixture = corpus::read(&testdata().join("records/holes/fixture.bin")).unwrap();
    array(field(fixture.as_dict().unwrap(), "cases"))
        .items
        .clone()
}

fn field<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

fn dict<'a>(parent: &'a Dict, key: &str) -> &'a Dict {
    field(parent, key).as_dict().unwrap()
}

fn array(value: &Variant) -> &Array {
    let Variant::Array(array) = value else {
        panic!("expected array");
    };
    array
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(value) = value else {
        panic!("expected integer");
    };
    value.value()
}

fn reference(case: &Dict, course: &Course) -> ReferenceHoleRecord {
    let input = dict(case, "input");
    let id = field(case, "id").as_str().unwrap();
    let output = dict(case, "output");
    let Variant::PackedByteArray(bytes) = field(output, "bytes") else {
        panic!("{id}: expected record bytes");
    };
    let course_id = field(dict(input, "course"), "id").as_str().unwrap();
    let record = decode_hole(bytes, course_id).unwrap_or_else(|error| panic!("{id}: {error}"));
    assert_eq!(
        record.dictionary(),
        dict(output, "decoded"),
        "{id}: decoded Variant dictionary"
    );
    assert_eq!(record.setup.course_id, course.data().id, "{id}: course id");
    assert_eq!(
        record.setup.course_revision,
        course.revision(),
        "{id}: course revision"
    );
    assert_eq!(
        field(dict(input, "course"), "revision").as_str(),
        Some(course.revision()),
        "{id}: fixture revision"
    );
    record
}

#[test]
fn format_four_decodes_every_reference_record() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let cases = fixture();
    assert_eq!(cases.len(), 30);
    for case in &cases {
        let case = case.as_dict().unwrap();
        let id = field(case, "id").as_str().unwrap();
        let input = dict(case, "input");
        let record = reference(case, &course);
        let settings = Profile::from_dict(dict(input, "profile"));
        assert!(settings.problems.is_empty(), "{id}");
        assert_eq!(&settings.to_dict(), dict(input, "profile"), "{id}");
        assert_eq!(record.setup.settings, settings.settings, "{id}");
        assert_eq!(
            record.setup.settings.info_mode as i64,
            integer(field(input, "info_mode")),
            "{id}"
        );

        let standard = HoleSetup::standard(&course, record.setup.hole, record.setup.seed);
        let built = standard.to_dict();
        let expected = dict(input, "setup");
        let recorded_setup = record.setup.to_dict();
        let recorded_setup = Dict::new(recorded_setup.entries()[1..].to_vec());
        assert_eq!(
            first_difference(
                &Variant::Dict(expected.clone()),
                &Variant::Dict(recorded_setup)
            ),
            None,
            "{id}: decoded setup"
        );
        for (key, value) in expected.entries() {
            let key = key.as_str().unwrap();
            let actual = field(&built, key);
            if key == "wind_mps" {
                let (Variant::Vector3(actual), Variant::Vector3(expected)) = (actual, value) else {
                    panic!("{id}: expected wind vector");
                };
                for (actual, expected) in actual.values.into_iter().zip(expected.values) {
                    let tolerance = expected.abs().max(1.0) * (1.0 / 8_388_608.0);
                    assert!(
                        (actual - expected).abs() <= tolerance,
                        "{id}: wind {actual} vs {expected}"
                    );
                }
            } else if key == "pressure_pa" {
                let (Variant::Float(actual), Variant::Float(expected)) = (actual, value) else {
                    panic!("{id}: expected station pressure");
                };
                let ulps = actual
                    .value()
                    .to_bits()
                    .abs_diff(expected.value().to_bits());
                assert!(ulps <= 4, "{id}: station pressure differs by {ulps} ULP");
            } else {
                assert_eq!(actual, value, "{id}: setup.{key}");
            }
        }
        let world = standard.world(&course, standard.tee_time_s).unwrap();
        assert_eq!(world.clock_s(), standard.tee_time_s, "{id}");
        assert_eq!(
            world.cup_position(),
            standard.cup(&course).map(|cup| cup.position),
            "{id}"
        );
    }
    println!(
        "Decoded 30 format 4 records with exact Variant equality and checked 30 standard setups."
    );
}

fn exact_record(reference: &ReferenceHoleRecord) -> HoleRecord {
    let mut record = HoleRecord::begin(
        reference.setup.clone(),
        HoleRecord::agent_driver("golf play"),
    );
    record.entries = reference
        .entries
        .iter()
        .map(|entry| match entry {
            ReferenceEntry::Stroke(facts) => {
                let execution = facts.execution;
                Entry::Stroke(Box::new(StrokeEntry {
                    facts: (**facts).clone(),
                    swing: StrokeEntry::swing_dict(Swing::Exact {
                        power: execution.power,
                        timing_fault: execution.timing_fault,
                        strike_m: execution.strike_m,
                    }),
                    address_usec: Some(250_000),
                }))
            }
            ReferenceEntry::Decision(decision) => Entry::Decision(decision.clone()),
        })
        .collect();
    record
}

#[test]
fn format_five_round_trips_every_reference_record() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let cases = fixture();
    assert_eq!(cases.len(), 30);
    let mut strokes = 0;
    let mut decisions = 0;
    for case in &cases {
        let case = case.as_dict().unwrap();
        let id = field(case, "id").as_str().unwrap();
        let reference = reference(case, &course);
        let record = exact_record(&reference);
        let bytes = record.to_bytes().unwrap();
        let reread = HoleRecord::from_bytes(&bytes).unwrap_or_else(|error| panic!("{id}: {error}"));
        reread.check_course(&course).unwrap();
        assert_eq!(reread.to_bytes().unwrap(), bytes, "{id}: different bytes");
        assert_eq!(reread.setup, reference.setup, "{id}: changed setup");
        let wire = decode_envelope(&bytes).unwrap();
        let entries = &array(field(&wire, "entries")).items;
        let original = &array(field(reference.dictionary(), "entries")).items;
        assert_eq!(entries.len(), original.len(), "{id}: entry count");
        for (index, (entry, old)) in entries.iter().zip(original).enumerate() {
            let wire_entry = entry.as_dict().unwrap();
            let shared = Variant::Dict(Dict::new(
                wire_entry
                    .entries()
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), Some("swing" | "address_usec")))
                    .cloned()
                    .collect(),
            ));
            assert_eq!(
                first_difference(old, &shared),
                None,
                "{id}: entries[{index}]"
            );
            match &reread.entries[index] {
                Entry::Stroke(stroke) => {
                    strokes += 1;
                    assert_eq!(stroke.address_usec, Some(250_000));
                    assert_eq!(dict(wire_entry, "swing").get("mode"), Some(&"exact".into()));
                    for value in [
                        field(wire_entry, "aim"),
                        field(wire_entry, "rest"),
                        field(dict(wire_entry, "lie"), "spot"),
                    ] {
                        assert!(
                            matches!(value, Variant::Vector3(v) if v.width == FloatWidth::Double)
                        );
                    }
                }
                Entry::Decision(_) => {
                    decisions += 1;
                    assert!(
                        matches!(field(wire_entry, "footprint"), Variant::PackedVector2Array(v) if v.width == FloatWidth::Double)
                    );
                    assert!(
                        matches!(field(dict(wire_entry, "ruling"), "rest"), Variant::Vector3(v) if v.width == FloatWidth::Double)
                    );
                }
            }
        }
    }
    assert_eq!((strokes, decisions), (124, 6));
    println!(
        "Round-tripped 30 format 5 records byte for byte with 124 exact swings and 6 decisions."
    );
}

fn first_record() -> HoleRecord {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let cases = fixture();
    exact_record(&reference(cases[0].as_dict().unwrap(), &course))
}

fn extensions(dict: &Dict) -> Dict {
    let extension = Variant::Vector2(Components {
        values: [1.5, -3.0],
        width: FloatWidth::Single,
    });
    Dict::new(
        [("extension_at_start".into(), extension)]
            .into_iter()
            .chain(dict.entries().iter().cloned())
            .chain([("extension_at_end".into(), Variant::Bool(true))])
            .collect(),
    )
}

#[test]
fn driver_and_swing_extensions_keep_their_order_and_values() {
    let mut record = first_record();
    record.driver = extensions(&record.driver);
    let [
        Entry::Stroke(first),
        Entry::Stroke(second),
        Entry::Stroke(third),
        ..,
    ] = record.entries.as_mut_slice()
    else {
        panic!("expected three strokes at the start of the record");
    };
    let strike = Vec2::new(0.001_234_567_890_123, -0.009_876_543_210_987);
    first.swing = extensions(&StrokeEntry::swing_dict(Swing::Exact {
        power: 0.875,
        timing_fault: -0.125,
        strike_m: Some(strike),
    }));
    second.swing = extensions(&StrokeEntry::pressed_swing([
        SwingPress::Press(1_001),
        SwingPress::Steer {
            usec: 1_502,
            delta_m: strike,
        },
        SwingPress::Press(2_002),
        SwingPress::Press(3_003),
    ]));
    third.swing = extensions(&StrokeEntry::swing_dict(Swing::Skill(Hands::HCP15)));
    let bytes = record.to_bytes().unwrap();
    let reread = HoleRecord::from_bytes(&bytes).unwrap();
    assert_eq!(reread.driver, record.driver);
    for (actual, expected) in reread.entries.iter().zip(&record.entries) {
        if let (Entry::Stroke(actual), Entry::Stroke(expected)) = (actual, expected) {
            assert_eq!(actual.swing, expected.swing);
        }
    }
    let Entry::Stroke(first) = &reread.entries[0] else {
        panic!("expected stroke");
    };
    assert_eq!(
        field(&first.swing, "strike_m"),
        &Variant::Vector2(Components {
            values: [strike.x, strike.y],
            width: FloatWidth::Double,
        })
    );
    assert_eq!(reread.to_bytes().unwrap(), bytes);
    println!(
        "Unknown driver and swing keys kept their order and values for exact, pressed and skill swings."
    );
}

fn frame(dict: Dict) -> Vec<u8> {
    let body = variant::encode(&Variant::Dict(dict)).unwrap();
    let mut bytes = b"LLHOLE01".to_vec();
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

#[test]
fn incompatible_and_malformed_records_are_refused() {
    let mut record = first_record();
    let bytes = record.to_bytes().unwrap();
    let mut damaged = bytes.clone();
    damaged[0] = b'X';
    assert_eq!(HoleRecord::from_bytes(&damaged), Err(Refusal::WrongMagic));
    for length in [8, 11, bytes.len() - 1] {
        assert_eq!(
            HoleRecord::from_bytes(&bytes[..length]),
            Err(Refusal::LengthMismatch)
        );
    }
    damaged = bytes.clone();
    damaged.push(0);
    assert_eq!(
        HoleRecord::from_bytes(&damaged),
        Err(Refusal::LengthMismatch)
    );
    assert_eq!(
        HoleRecord::from_bytes(&frame(Dict::default())),
        Err(Refusal::WrongFormat)
    );

    for version in ["1", "2", "3", "4", "6", "future"] {
        *record.versions.get_mut("record").unwrap() = version.into();
        let bytes = record.to_bytes().unwrap();
        assert_eq!(
            HoleRecord::from_bytes(&bytes),
            Err(Refusal::UnsupportedFormat(version.into()))
        );
        if matches!(version, "1" | "2" | "3") {
            assert_eq!(
                decode_hole(&bytes, "test-course"),
                Err(Refusal::UnsupportedFormat(version.into()))
            );
        }
    }
    *record.versions.get_mut("record").unwrap() = "5".into();

    for (model, current) in [
        ("release", env!("CARGO_PKG_VERSION")),
        ("commit", option_env!("GOLF_SIM_COMMIT").unwrap_or("dev")),
        ("physics", golf_sim_physics::tunables::VERSION),
        ("tuning", golf_sim_stroke::tuning::VERSION),
        ("rules", "unversioned"),
    ] {
        assert_eq!(record.versions.get(model), Some(&current.into()));
        *record.versions.get_mut(model).unwrap() = "incompatible".into();
        assert_eq!(
            HoleRecord::from_bytes(&record.to_bytes().unwrap()),
            Err(Refusal::IncompatibleVersion {
                model,
                recorded: "incompatible".into(),
                current
            })
        );
        *record.versions.get_mut(model).unwrap() = current.into();
    }

    let mut envelope = decode_envelope(&bytes).unwrap();
    let Variant::Array(entries) = envelope.get_mut("entries").unwrap() else {
        panic!("expected entries");
    };
    let Variant::Dict(first) = &mut entries.items[0] else {
        panic!("expected entry");
    };
    let Variant::Vector3(rest) = first.get_mut("rest").unwrap() else {
        panic!("expected rest position");
    };
    rest.width = FloatWidth::Single;
    assert!(matches!(
        HoleRecord::from_bytes(&frame(envelope)),
        Err(Refusal::Invalid(Error::Malformed { path, .. })) if path == "entries[0].rest"
    ));

    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    record.setup.course_revision = "different".into();
    assert_eq!(
        record.check_course(&course),
        Err(Refusal::CourseRevision {
            recorded: "different".into(),
            supplied: course.revision().into(),
        })
    );
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke");
    };
    stroke.address_usec = None;
    assert_eq!(
        record.to_bytes(),
        Err(Refusal::Invalid(Error::MissingKey {
            path: "entries[0].address_usec".into(),
        }))
    );
    record.driver = HoleRecord::console_driver();
    assert!(HoleRecord::from_bytes(&record.to_bytes().unwrap()).is_ok());
    println!(
        "Verified typed refusals for framing, unsupported formats, incompatible versions, course revisions and invalid entry fields."
    );
}

#[test]
fn profiles_round_trip_settings_presets_and_stock() {
    let mut settings = PlaySettings::default();
    assert!(settings.apply(PlaySetting::InfoMode, 1.0));
    assert!(!settings.apply(PlaySetting::InfoMode, 2.0));
    assert!(settings.apply(PlaySetting::Pace, 5.0));
    assert_eq!(settings.pace, 2.0);
    assert_eq!(settings.info_mode, Mode::Tour);
    assert!(!settings.shows_slopes());
    assert_eq!(settings.window_scale(), 1.0);
    assert!(settings.apply(PlaySetting::TourSlopes, 1.0));
    assert!(settings.shows_slopes());
    assert_eq!(settings.strike_variant, StrikeVariant::A);
    assert_eq!(PlaySettings::default().window_scale(), 1.2);

    let mut profile = Profile {
        settings,
        starter_choice_made: true,
        presets: golf_sim_stroke::address::Preset::starters().into(),
        ..Profile::default()
    };
    profile
        .stock
        .insert(ClubId::Driver, Address::stock(ClubId::Driver));
    profile.range_choices.insert("target".into(), 3);
    let reread = Profile::from_dict(&profile.to_dict());
    assert_eq!(reread, profile);
    assert_eq!(
        reread.presets_for(Group::Woods).count(),
        profile.presets_for(Group::Woods).count()
    );
    assert_eq!(
        reread.stock_for(ClubId::Putter),
        Address::stock(ClubId::Putter)
    );
    assert_eq!(
        PlaySettings::from_dict(&Dict::default()),
        PlaySettings::default()
    );

    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut standard = HoleSetup::standard(&course, 1, Seed(i64::MIN + 1));
    standard.settings = settings;
    let bytes = HoleRecord::begin(standard.clone(), HoleRecord::console_driver())
        .to_bytes()
        .unwrap();
    assert_eq!(HoleRecord::from_bytes(&bytes).unwrap().setup, standard);
}
