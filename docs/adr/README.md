# Architecture decision records

An ADR records one decision that shapes the core. It says what was decided, why, what lost, and what the decision costs. [`docs/design/core.md`](../design/core.md) describes how the core works now. An ADR explains why it works that way.

## Index

| ADR | Decision | Status | Date |
| --- | --- | --- | --- |
| [0001](0001-every-decision-goes-through-one-apply.md) | Every decision goes through one `apply`, for people, agents and replays | Accepted, amended by 0005 | 2026-11-26 |
| [0002](0002-one-pure-rust-maths-library-on-every-platform.md) | One pure-Rust maths library on every platform | Accepted, amended by 0004 | 2026-11-26 |
| [0003](0003-godot-variant-bytes-for-courses-records-and-digests.md) | Godot Variant bytes for courses, records and digests | Accepted | 2026-11-26 |
| [0004](0004-checked-against-measured-golf-not-the-earlier-implementation-s-bits.md) | Checked against measured golf, not against the earlier implementation's bits | Accepted | 2026-11-26 |
| [0005](0005-tests-and-agents-state-a-swing-or-use-a-seeded-profile.md) | Tests and agents state a swing exactly or use a seeded skill profile | Accepted | 2026-11-26 |
| [0006](0006-every-result-can-be-replayed-and-explained.md) | Every result can be replayed and explained | Accepted | 2026-11-26 |

## When to write one

Write an ADR when a decision changes a crate boundary, a public type, a file format, a determinism rule or the release process, or when it would cost more than a day to reverse. Commit it with the change it records, and add its row to the index.

## Template

```markdown
# NNNN. The decision, as a short noun phrase

- Status: Proposed, Accepted, or Superseded by NNNN
- Date: YYYY-MM-DD

## Context

The forces that led to the decision, with the measurements.

## Decision

What the core does.

## Alternatives considered

- **The alternative.** Why it lost.

## Consequences

- What the decision makes cheaper or possible, and what it costs.
```
