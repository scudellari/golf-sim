//! Godot's GCPF zstd block container and the course manifest.

use std::{fs, io::Read, path::Path};

use ruzstd::{
    decoding::StreamingDecoder,
    encoding::{CompressionLevel, compress_to_vec},
};
use serde::Deserialize;

use crate::{Error, digest::sha256_hex};

const MAGIC: &[u8; 4] = b"GCPF";
const MODE_ZSTD: u32 = 2;

pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let rest = bytes
        .strip_prefix(MAGIC)
        .and_then(|bytes| bytes.strip_suffix(MAGIC))
        .ok_or_else(|| malformed("GCPF", "missing header or footer magic"))?;
    let (header, rest) = rest
        .split_at_checked(12)
        .ok_or_else(|| malformed("GCPF", "truncated header"))?;
    if word(&header[..4]) != MODE_ZSTD {
        return Err(malformed("GCPF.mode", "expected zstd"));
    }
    let block_size = word(&header[4..8]) as usize;
    let total = word(&header[8..12]) as usize;
    let count = block_count(total, block_size)?;
    let table_size = count
        .checked_mul(4)
        .ok_or_else(|| malformed("GCPF", "block table too large"))?;
    let (sizes, mut frames) = rest
        .split_at_checked(table_size)
        .ok_or_else(|| malformed("GCPF", "truncated block table"))?;

    let mut out = Vec::new();
    for (index, &size) in sizes.as_chunks::<4>().0.iter().enumerate() {
        let error = |message: &str| malformed(format!("GCPF.blocks[{index}]"), message);
        let size = u32::from_le_bytes(size) as usize;
        let (frame, rest) = frames
            .split_at_checked(size)
            .ok_or_else(|| error("truncated frame"))?;
        frames = rest;

        let expected = block_size.min(total - out.len());
        let before = out.len();
        let mut decoder = StreamingDecoder::new(frame).map_err(|err| error(&err.to_string()))?;
        decoder
            .by_ref()
            .take(expected as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|err| error(&err.to_string()))?;
        if out.len() - before != expected {
            return Err(error("incorrect uncompressed size"));
        }
        let (unread, frame) = decoder.into_parts();
        if !unread.is_empty() || !frame.is_finished() {
            return Err(error("expected one complete zstd frame"));
        }
        if frame
            .get_checksum_from_data()
            .is_some_and(|checksum| Some(checksum) != frame.get_calculated_checksum())
        {
            return Err(error("incorrect zstd checksum"));
        }
    }
    if !frames.is_empty() {
        return Err(malformed("GCPF", "trailing bytes after blocks"));
    }
    Ok(out)
}

/// Writes a GCPF container for tests. The course baker owns production bundles.
pub fn encode(bytes: &[u8], block_size: u32) -> Result<Vec<u8>, Error> {
    let total = u32::try_from(bytes.len()).map_err(|_| malformed("GCPF", "payload too large"))?;
    let count = block_count(bytes.len(), block_size as usize)?;
    let mut out = Vec::from(*MAGIC);
    out.extend(MODE_ZSTD.to_le_bytes());
    out.extend(block_size.to_le_bytes());
    out.extend(total.to_le_bytes());
    let mut frames = Vec::new();
    let block_size = block_size as usize;
    for index in 0..count {
        let offset = index * block_size;
        let size = block_size.min(bytes.len() - offset);
        let frame = compress_to_vec(&bytes[offset..offset + size], CompressionLevel::Fastest);
        let size = u32::try_from(frame.len())
            .map_err(|_| malformed("GCPF", "compressed frame too large"))?;
        out.extend(size.to_le_bytes());
        frames.extend(frame);
    }
    out.extend(frames);
    out.extend(MAGIC);
    Ok(out)
}

fn block_count(total: usize, block_size: usize) -> Result<usize, Error> {
    // Godot always writes a last block, even when the payload ends on a block boundary.
    total
        .checked_div(block_size)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| malformed("GCPF.block_size", "invalid block size or count"))
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: i64,
    pub bake_version: String,
    pub course_id: String,
    pub course_revision: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self, Error> {
        let file = dir.join("manifest.json");
        let manifest: Self = serde_json::from_slice(&read(&file)?)
            .map_err(|error| malformed(file.display().to_string(), error.to_string()))?;
        if manifest.format_version != 1 {
            return Err(Error::UnknownVersion {
                what: "course bundle",
                version: manifest.format_version,
            });
        }
        Ok(manifest)
    }

    /// Load the payload, verify the file hashes and check the decompressed revision.
    pub fn read_payload(&self, dir: &Path) -> Result<Vec<u8>, Error> {
        let mut payload = None;
        for file in &self.files {
            let mut path = dir.to_path_buf();
            for component in file.path.split('/') {
                if matches!(component, "" | "." | "..") || component.contains(['\\', ':']) {
                    return Err(malformed(&file.path, "invalid manifest file path"));
                }
                path.push(component);
            }
            let bytes = read(&path)?;
            if sha256_hex(&bytes) != file.sha256 {
                return Err(malformed(&file.path, "SHA-256 differs from the manifest"));
            }
            if file.path == "course.payload" && payload.replace(bytes).is_some() {
                return Err(malformed("manifest.json.files", "duplicate course.payload"));
            }
        }
        let payload = payload.ok_or_else(|| Error::MissingKey {
            path: "manifest.json.files[\"course.payload\"]".into(),
        })?;
        let bytes = decode(&payload)?;
        let actual = sha256_hex(&bytes);
        if actual != self.course_revision {
            return Err(Error::RevisionMismatch {
                expected: self.course_revision.clone(),
                actual,
            });
        }
        Ok(bytes)
    }
}

fn read(file: &Path) -> Result<Vec<u8>, Error> {
    fs::read(file).map_err(|error| Error::Io {
        file: file.display().to_string(),
        message: error.to_string(),
    })
}

fn malformed(path: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
}
