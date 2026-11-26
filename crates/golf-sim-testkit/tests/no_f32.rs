//! The core uses 64-bit floats outside the codecs.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};

fn uses_f32(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Group(group) => uses_f32(group.stream()),
        TokenTree::Ident(ident) => ident == "f32" || ident == "r#f32",
        TokenTree::Literal(literal) => {
            let literal = literal.to_string();
            literal.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && !literal.starts_with("0x")
                && literal.ends_with("f32")
        }
        TokenTree::Punct(_) => false,
    })
}

fn check_sources(dir: &Path, codecs: &[PathBuf]) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            check_sources(&path, codecs);
        } else if path.extension().is_some_and(|ext| ext == "rs") && !codecs.contains(&path) {
            let source = fs::read_to_string(&path).unwrap();
            let tokens = source
                .parse()
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert!(!uses_f32(tokens), "{} uses f32", path.display());
        }
    }
}

#[test]
fn the_core_uses_no_f32_outside_the_codecs() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let codecs = [
        crates.join("golf-sim-base/src/variant.rs"),
        crates.join("golf-sim-base/src/container.rs"),
    ];
    for entry in fs::read_dir(&crates).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir()
            || matches!(
                entry.file_name().to_str(),
                Some("golf-sim-cli" | "golf-sim-godot" | "golf-sim-testkit")
            )
        {
            continue;
        }
        check_sources(&entry.path().join("src"), &codecs);
    }
}

#[test]
fn the_scan_detects_f32_types_and_literals() {
    for source in [
        "fn example(value: f32) {}",
        "fn example() { let _ = 1.0_f32; }",
        "fn example() { let _ = 1f32; }",
    ] {
        assert!(uses_f32(source.parse().unwrap()), "{source}");
    }
    let source = r#"
        // f32 in a comment
        fn example(f32_name: f64) { let _ = ("f32", 0xf32, 1.0_f64); }
    "#;
    assert!(!uses_f32(source.parse().unwrap()));
}
