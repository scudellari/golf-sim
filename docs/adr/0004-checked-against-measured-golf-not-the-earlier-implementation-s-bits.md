# 0004. Checked against measured golf, not against the earlier implementation's bits

- Status: Accepted
- Date: 2026-11-26

## Context

The core began as a port of a GDScript implementation. The first plan proved each unit bit for bit against that implementation, in a test-only `reference` configuration that used single-precision vectors and the platform's maths library, and then stored the default build's outputs as twins. The owner rejected that goal on 2026-11-26: "I care less about matching GDScript exactly and more about getting the simulation CORRECT to what it should be to match real physics."

The reference implementation records residuals against published measurements.

## Decision

- The core uses the portable `libm` maths and 64-bit floats from the first unit. No `reference` feature exists, and no twins are stored.
- A physics unit is done when it passes its measured-data targets. A target passes when the core is within the source's tolerance, or no further from the measurement than the earlier implementation, plus the layer's tolerance.
- The earlier implementation stays as a coarse oracle until it is retired. Each layer compares against its recorded data within a stated tolerance, which catches porting slips without demanding the same bits.
- A model change may fix a known residual. It raises the physics version, shows the residual report before and after, and lists the oracle cases it moves.
- The licence ledger records sources and their terms. Committed data includes openly licensed files with attribution and cited figures.
- Determinism across platforms is a separate requirement, and it still holds.

## Alternatives considered

- **Bit-identity in a test-only configuration.** It catches every porting slip exactly, but it makes the earlier implementation, with its residuals, the definition of correct. It also needed the platform's maths library in the core, against the determinism rules.
- **Measured data alone.** It would let a porting slip through wherever no measurement reaches, such as the rules, the console and the generator.

## Consequences

- The core can improve on the earlier implementation, and the residual report shows each improvement.
- A porting slip smaller than a layer's tolerance goes unnoticed. The tolerances start narrow and are widened only with evidence.
- A discrete result near a threshold, such as a putt on the lip, can differ from the earlier implementation. The comparisons list such cases instead of failing on them.
- [ADR 0002](0002-one-pure-rust-maths-library-on-every-platform.md) loses its `reference` feature.
