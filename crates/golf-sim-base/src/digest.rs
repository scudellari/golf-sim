//! SHA-256 over the Variant representation of a value.

use std::fmt::Write;

use sha2::{Digest, Sha256};

use crate::Error;
use crate::variant::{self, Dict, Variant};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

pub fn digest(value: &Variant) -> Result<String, Error> {
    Ok(sha256_hex(&variant::encode(value)?))
}

/// Pass the stored fields in declaration order. Nested objects use the same conversion.
pub fn object<'a>(fields: impl IntoIterator<Item = (&'a str, Variant)>) -> Variant {
    Variant::Dict(Dict::new(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    ))
}
