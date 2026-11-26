# 0003. Godot Variant bytes for courses, records and digests

- Status: Accepted
- Date: 2026-11-26

## Context

The course baker writes a course as one Godot Variant inside Godot's zstd container, and the course's revision is the SHA-256 of the Variant bytes. The reference implementation's records, its round fingerprints and its cached course analysis are Variant bytes too. Godot's JSON loses doubles: a probe found 53,062 of 200,000 random doubles changed after a round trip. A spike read two real bundles of 5.9 MB and 76.4 MB with a 200-line Rust reader. Both revisions matched the manifests, and re-encoding the decoded values gave back every byte.

## Decision

- `golf-sim-base` reads and writes Godot 4.7's binary Variant encoding exactly, with its width rules for ints and floats and an ordered dictionary.
- Courses, records, fixtures and digests use it. The `golf` program's JSON protocol is separate and uses `serde_json` with exact float round trips.

## Alternatives considered

- **A new course format that the core defines.** The baker would write two artifacts with two revisions, and the two could disagree.
- **JSON records.** Godot's JSON changes doubles, so a record would replay a different swing.

## Consequences

- A game built on Godot can read the core's records and fixtures with `bytes_to_var`.
- The codec must track Godot's encoding. The bundle's format version and the record's format version catch a change.
