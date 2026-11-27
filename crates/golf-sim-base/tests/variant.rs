use std::{fs, path::Path, path::PathBuf, time::SystemTime};

use golf_sim_base::{
    Error, container,
    corpus::{self, Tolerance},
    digest::{digest, object, sha256_hex},
    variant::{self, Array, Components, ContainerType, Dict, FloatWidth, Variant},
};
use serde_json::json;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn every_snapshot_matches_the_reference_bytes_and_digest() {
    let fixture = corpus::read(&testdata().join("variant/digests/fixture.bin")).unwrap();
    let Variant::Array(cases) = fixture.as_dict().unwrap().get("cases").unwrap() else {
        panic!("expected fixture cases");
    };
    assert!(!cases.items.is_empty());
    for case in &cases.items {
        let case = case.as_dict().unwrap();
        let id = case.get("id").unwrap().as_str().unwrap();
        assert_eq!(case.get("op").unwrap().as_str(), Some("snapshot"), "{id}");
        let value = case.get("value").unwrap();
        let Variant::PackedByteArray(bytes) = case.get("bytes").unwrap() else {
            panic!("{id}: expected encoded snapshot");
        };
        let expected_digest = case.get("digest").unwrap().as_str().unwrap();
        assert_eq!(&variant::encode(value).unwrap(), bytes, "{id}");
        assert_eq!(
            variant::encode(&variant::decode(bytes).unwrap()).unwrap(),
            *bytes,
            "{id}"
        );
        assert_eq!(sha256_hex(bytes), expected_digest, "{id}");
        assert_eq!(digest(value).unwrap(), expected_digest, "{id}");
    }
    println!(
        "{} digest cases matched their Variant bytes and SHA-256",
        cases.items.len()
    );
}

#[test]
fn object_digest_preserves_declaration_order() {
    let fields = [("z", 17_i64.into()), ("a", Variant::Bool(true))];
    let value = object(fields.clone());
    let expected = variant::decode(&words(&[
        27,
        2,
        4,
        1,
        b'z'.into(),
        2,
        17,
        4,
        1,
        b'a'.into(),
        1,
        1,
    ]))
    .unwrap();
    assert_eq!(digest(&value), digest(&expected));
    assert_ne!(digest(&value), digest(&object(fields.into_iter().rev())));
}

#[test]
fn typed_containers_preserve_their_header_and_metadata() {
    let bytes = words(&[
        0x0005_001b,
        4,
        28,
        1,
        4,
        1,
        b'v'.into(),
        0x0001_001c,
        2,
        2,
        2,
        7,
        2,
        9,
    ]);
    let value = variant::decode(&bytes).unwrap();
    let dict = value.as_dict().unwrap();
    assert_eq!(dict.key_type, ContainerType::Builtin(4));
    assert_eq!(dict.value_type, ContainerType::Builtin(28));
    assert_eq!(
        dict.get("v"),
        Some(&Variant::Array(Array::typed(
            ContainerType::Builtin(2),
            vec![7_i64.into(), 9_i64.into()],
        )))
    );
    assert_eq!(variant::encode(&value).unwrap(), bytes);

    for (kind, element_type) in [
        (2, ContainerType::Class("Node".into())),
        (3, ContainerType::Script("Node".into())),
    ] {
        let bytes = words(&[28 | (kind << 16), 4, u32::from_le_bytes(*b"Node"), 0]);
        let value = variant::decode(&bytes).unwrap();
        assert_eq!(value, Variant::Array(Array::typed(element_type, vec![])));
        assert_eq!(variant::encode(&value).unwrap(), bytes);
    }
}

#[test]
fn variant_preserves_stored_widths_and_float_bits() {
    for case in [
        words(&[0x0001_0002, 7, 0]),
        words(&[0x0001_0003, 0, 0x3ff0_0000]),
        words(&[3, 0xff80_0001]),
        words(&[32, 3, 0x8000_0000, 1, 0x7f80_0001]),
        words(&[0x0001_0024, 1, 1, 0, 0, 0x8000_0000, 0, 0x7ff8_0000]),
    ] {
        let value = variant::decode(&case).unwrap();
        assert_eq!(variant::encode(&value).unwrap(), case);
        for end in 0..case.len() {
            assert!(variant::decode(&case[..end]).is_err());
        }
    }
    assert_eq!(variant::encode(&3_i64.into()).unwrap(), words(&[2, 3]));
    assert_eq!(
        variant::encode(&(-0.0).into()).unwrap(),
        words(&[3, 0x8000_0000])
    );
    assert_eq!(
        variant::encode(&f64::from_bits(1).into()).unwrap(),
        words(&[0x0001_0003, 1, 0])
    );
}

#[test]
fn comparer_names_the_first_edited_value_and_applies_per_path_tolerances() {
    let expected = dict([(
        "cases",
        Variant::Array(Array::new(vec![
            dict([(
                "out",
                Variant::Vector3(Components {
                    width: FloatWidth::Double,
                    values: [1.0, 100.0, 3.0],
                }),
            )]);
            13
        ])),
    )]);
    let mut actual = expected.clone();
    let Variant::Dict(root) = &mut actual else {
        unreachable!();
    };
    let Some(Variant::Array(cases)) = root.get_mut("cases") else {
        unreachable!();
    };
    let Variant::Dict(case) = &mut cases.items[12] else {
        unreachable!();
    };
    let Some(Variant::Vector3(out)) = case.get_mut("out") else {
        unreachable!();
    };
    out.values[1] += 0.01;
    out.values[2] = 4.0;
    assert_eq!(corpus::first_difference(&expected, &expected), None);
    let path = corpus::first_difference(&expected, &actual).unwrap();
    assert_eq!(path, "cases[12].out.y");
    println!("first difference: {path}");

    for tolerance in [
        Tolerance {
            absolute: 0.02,
            relative: 0.0,
        },
        Tolerance {
            absolute: 0.0,
            relative: 0.0002,
        },
    ] {
        let difference = corpus::first_difference_with(&expected, &actual, |path| {
            (path == "cases[12].out.y").then_some(tolerance)
        });
        assert_eq!(difference.as_deref(), Some("cases[12].out.z"));
    }
    assert_eq!(
        corpus::first_difference(&Variant::from(0.0), &Variant::from(-0.0)).as_deref(),
        Some("$")
    );
    let nan = Variant::from(f64::from_bits(0x7ff8_0000_0000_0001));
    assert_eq!(corpus::first_difference(&nan, &nan), None);
}

#[test]
fn corpus_rejects_invalid_size_and_case_id() {
    let fixture = dict([
        ("format", 1_i64.into()),
        ("layer", "example".into()),
        (
            "oracle",
            dict([
                ("versions", dict([])),
                ("commit", "abc".into()),
                ("hash", "def".into()),
            ]),
        ),
        (
            "cases",
            Variant::Array(Array::new(vec![dict([("id", 1_i64.into())])])),
        ),
    ]);
    let payload = variant::encode(&fixture).unwrap();
    let mut bytes = b"GSCORP01".to_vec();
    bytes.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(payload);
    assert!(corpus::decode(&bytes[..bytes.len() - 1]).is_err());
    assert!(matches!(
        corpus::decode(&bytes),
        Err(Error::WrongType { path, .. }) if path == "cases[0].id"
    ));
}

#[test]
fn gcpf_handles_empty_and_full_final_blocks_and_detects_damage() {
    for bytes in [b"".as_slice(), b"abcd", b"abcde"] {
        let encoded = container::encode(bytes, 4).unwrap();
        assert_eq!(container::decode(&encoded).unwrap(), bytes);
        assert!(container::decode(&encoded[..encoded.len() - 1]).is_err());
        let mut corrupt = encoded;
        corrupt[8..12].fill(0);
        assert!(container::decode(&corrupt).is_err());
    }
}

#[test]
fn small_course_decodes_and_matches_its_manifest() {
    let dir = TempBundle::new();
    let payload = variant::encode(&dict([
        ("format_version", 1_i64.into()),
        ("course", dict([("id", "test-course".into())])),
    ]))
    .unwrap();
    let compressed = container::encode(&payload, 12).unwrap();
    fs::write(dir.0.join("course.payload"), &compressed).unwrap();
    let mut manifest = json!({
        "format": "course",
        "format_version": 1,
        "bake_version": "1.0.0",
        "course_id": "test-course",
        "course_revision": sha256_hex(&payload),
        "files": [{"path": "course.payload", "sha256": sha256_hex(&compressed)}]
    });
    let file = dir.0.join("manifest.json");
    fs::write(
        &file,
        serde_json::to_string_pretty(&manifest)
            .unwrap()
            .replace('\n', "\r\n"),
    )
    .unwrap();
    check_course(&dir.0.join(".").join("..").join(dir.0.file_name().unwrap()));

    let revision = manifest["course_revision"].clone();
    manifest["course_revision"] = json!("0".repeat(64));
    fs::write(&file, manifest.to_string()).unwrap();
    let parsed = container::Manifest::read(&dir.0).unwrap();
    assert!(matches!(
        parsed.read_payload(&dir.0),
        Err(Error::RevisionMismatch { actual, .. }) if json!(actual) == revision
    ));

    let mut damaged = compressed;
    damaged[0] ^= 1;
    fs::write(dir.0.join("course.payload"), damaged).unwrap();
    assert!(matches!(
        parsed.read_payload(&dir.0),
        Err(Error::Malformed { path, .. }) if path == "course.payload"
    ));

    manifest["format_version"] = json!(2);
    fs::write(&file, manifest.to_string()).unwrap();
    assert!(matches!(
        container::Manifest::read(&dir.0),
        Err(Error::UnknownVersion { version: 2, .. })
    ));
}

#[test]
fn reference_course_decodes_and_matches_its_manifest() {
    check_course(&testdata().join("courses/test-course"));
}

fn check_course(dir: &Path) {
    let manifest = container::Manifest::read(dir).unwrap();
    let bytes = manifest.read_payload(dir).unwrap();
    let value = variant::decode(&bytes).unwrap();
    assert!(value.as_dict().is_some());
    let revision = sha256_hex(&bytes);
    assert_eq!(revision, manifest.course_revision);
    assert_eq!(variant::encode(&value).unwrap(), bytes);
    println!(
        "course revision {revision} matched the manifest and all {} Variant bytes round-tripped",
        bytes.len()
    );
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|&word| word.to_le_bytes()).collect()
}

fn dict<const N: usize>(entries: [(&str, Variant); N]) -> Variant {
    Variant::Dict(Dict::new(
        entries
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    ))
}

struct TempBundle(PathBuf);

impl TempBundle {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("golf-sim-bundle-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempBundle {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
