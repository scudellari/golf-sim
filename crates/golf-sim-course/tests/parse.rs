use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use golf_sim_base::{
    Error, Vec2,
    container::{self, Manifest},
    digest::sha256_hex,
    variant::{self, Dict, Variant},
};
use golf_sim_course::course::{Course, SurfaceId};
use golf_sim_testkit::corpus;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn bundle_dir() -> PathBuf {
    testdata().join("courses/test-course")
}

#[test]
fn course_grids_match_reference_probes() {
    let path = testdata().join("course/grids/fixture.bin");
    let report = corpus::FixtureRun::read(&path, None, &[])
        .unwrap()
        .check()
        .unwrap();
    assert!(report.checked > 0);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!("material ids, kinds and indices matched exactly");
}

#[test]
fn surface_identifies_hole_tiles_property_and_uncovered_ground() {
    let course = Course::open(&bundle_dir()).unwrap();
    let points = [
        Vec2::new(50.0, 24.0),
        Vec2::new(450.0, 32.0),
        Vec2::new(16.0, 156.0),
    ];
    for (index, point) in points.into_iter().enumerate() {
        assert_eq!(course.surface_at(point).unwrap().0, SurfaceId::Tile(index));
    }
    let overlap = Vec2::new(400.0, 32.0);
    assert_eq!(course.surface_at(overlap).unwrap().0, SurfaceId::Tile(0));
    assert_eq!(
        course.surface_at(Vec2::ZERO).unwrap().0,
        SurfaceId::Property
    );
    assert!(course.surface_at(Vec2::new(-1.0, -1.0)).is_none());
    println!("surface ids matched for all hole tiles, the property and uncovered ground");
}

#[test]
fn crlf_manifest_and_dotted_path_open_the_same_course() {
    let source = bundle_dir();
    let canonical = Course::open(&source).unwrap();
    let dir = TempBundle::new();
    let copy = dir.0.join("course");
    fs::create_dir(&copy).unwrap();
    let manifest = fs::read_to_string(source.join("manifest.json")).unwrap();
    fs::write(copy.join("manifest.json"), manifest.replace('\n', "\r\n")).unwrap();
    fs::copy(source.join("course.payload"), copy.join("course.payload")).unwrap();

    let opened = Course::open(&copy.join(".").join("..").join("course")).unwrap();
    assert_eq!(opened.revision(), canonical.revision());
    assert!(opened == canonical);
    println!(
        "CRLF manifest and dotted path opened the same course at revision {}",
        opened.revision()
    );
}

#[test]
fn renamed_and_mistyped_keys_report_their_full_path() {
    let mut payload = decoded_payload();
    let hole = array_item(dict_field(&mut payload, "holes"), 2);
    let walk = std::mem::replace(dict_field(hole, "walk"), Variant::Nil);
    let error = Course::parse(&variant::encode(&payload).unwrap()).unwrap_err();
    assert!(matches!(&error, Error::WrongType { path, .. } if path == "holes[2].walk"));
    println!("mistyped key refused: {error}");

    let hole = array_item(dict_field(&mut payload, "holes"), 2);
    *dict_field(hole, "walk") = walk;
    let tee = array_item(dict_field(hole, "tees"), 0);
    let mut entries = tee.as_dict().unwrap().entries().to_vec();
    let (position, _) = entries
        .iter_mut()
        .find(|(key, _)| key.as_str() == Some("position"))
        .unwrap();
    *position = "renamed_position".into();
    *tee = Variant::Dict(Dict::new(entries));
    let bytes = variant::encode(&payload).unwrap();
    let dir = TempBundle::new();
    write_bundle(&dir.0, &bytes, "0.6.0", &sha256_hex(&bytes));

    let error = Course::open(&dir.0).unwrap_err();
    assert!(matches!(&error, Error::MissingKey { path } if path == "holes[2].tees[0].position"));
    println!("renamed key refused: {error}");
}

#[test]
fn versions_and_manifest_revision_are_checked() {
    let mut payload = decoded_payload();
    *dict_field(&mut payload, "format") = "unknown".into();
    let error = Course::parse(&variant::encode(&payload).unwrap()).unwrap_err();
    assert!(matches!(error, Error::Malformed { path, .. } if path == "format"));

    let mut payload = decoded_payload();
    *dict_field(&mut payload, "format_version") = 2_i64.into();
    let error = Course::parse(&variant::encode(&payload).unwrap()).unwrap_err();
    assert!(matches!(
        error,
        Error::UnknownVersion {
            what: "format_version",
            version: 2
        }
    ));

    *dict_field(&mut payload, "format_version") = 1_i64.into();
    *dict_field(&mut payload, "bake_version") = "99.0.0".into();
    let error = Course::parse(&variant::encode(&payload).unwrap()).unwrap_err();
    assert!(matches!(error, Error::Malformed { path, .. } if path == "bake_version"));

    let source = bundle_dir();
    let bytes = Manifest::read(&source)
        .unwrap()
        .read_payload(&source)
        .unwrap();
    let dir = TempBundle::new();
    write_bundle(&dir.0, &bytes, "99.0.0", &sha256_hex(&bytes));
    let error = Course::open(&dir.0).unwrap_err();
    assert!(matches!(error, Error::Malformed { path, .. } if path == "manifest.json.bake_version"));

    write_bundle(&dir.0, &bytes, "0.6.0", &"0".repeat(64));
    assert!(matches!(
        Course::open(&dir.0),
        Err(Error::RevisionMismatch { .. })
    ));
}

#[test]
fn drawing_values_preserve_variant_bytes() {
    let payload = decoded_payload();
    let course = Course::open(&bundle_dir()).unwrap();
    let drawing = &course.data().drawing;
    for (key, value) in [
        ("turf_tiles", Some(&drawing.turf_tiles)),
        ("forest", drawing.forest.as_ref()),
        ("wall_kit", drawing.wall_kit.as_ref()),
        ("wall_sections", drawing.wall_sections.as_ref()),
        ("wall_stones", drawing.wall_stones.as_ref()),
    ] {
        let encoded = |value: &Variant| variant::encode(value).unwrap();
        let expected = payload.as_dict().unwrap().get(key);
        assert_eq!(value.map(encoded), expected.map(encoded), "{key}");
    }
}

fn decoded_payload() -> Variant {
    let dir = bundle_dir();
    let bytes = Manifest::read(&dir).unwrap().read_payload(&dir).unwrap();
    variant::decode(&bytes).unwrap()
}

fn dict_field<'a>(value: &'a mut Variant, key: &str) -> &'a mut Variant {
    let Variant::Dict(dict) = value else {
        panic!("expected dictionary containing {key}");
    };
    dict.get_mut(key).unwrap()
}

fn array_item(value: &mut Variant, index: usize) -> &mut Variant {
    let Variant::Array(array) = value else {
        panic!("expected array");
    };
    &mut array.items[index]
}

fn write_bundle(dir: &Path, bytes: &[u8], bake: &str, revision: &str) {
    let compressed = container::encode(bytes, 4096).unwrap();
    let sha256 = sha256_hex(&compressed);
    fs::write(dir.join("course.payload"), compressed).unwrap();
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{
        "format": "links-lab-course",
        "format_version": 1,
        "bake_version": "{bake}",
        "course_id": "test-course",
        "course_revision": "{revision}",
        "files": [{{"path": "course.payload", "sha256": "{sha256}"}}]
    }}"#
        ),
    )
    .unwrap();
}

struct TempBundle(PathBuf);

impl TempBundle {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("golf-course-{}-{stamp}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempBundle {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
